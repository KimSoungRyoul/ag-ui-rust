//! The two objects an agent works through: the generator it builds once, and
//! the request processor the generator hands back once a renderer has said
//! what it can render.

use std::sync::Arc;

use serde_json::Value;

use super::calls::RendererCalls;
use super::capabilities::{AgentCapabilities, RendererCapabilities, resolve_catalogs};
use super::catalog::{CatalogConfig, CatalogDocument, check_catalogs};
use super::direct_json::DirectJson;
use super::format::{OutputFormat, RawPart, ResponseError, ResponsePart};
use super::functions::RendererMessageRouter;
use super::state::{RendererState, session_processor, validate_payload};
use super::stream::{ResponseStream, StreamOptions};
use crate::error::{Error, Result};
use crate::protocol::message::AgentMessage;
use crate::protocol::processor::MessageProcessor;
use crate::protocol::version::A2uiVersion;

/// What an agent knows about A2UI for its whole life: the catalogs it
/// registered, its examples, and the format it asks its model for.
///
/// Built once. For each request, [`A2uiGenerator::create_processor`]
/// negotiates with that request's renderer and returns the
/// [`A2uiRequestProcessor`] the request is served with. Creating a processor
/// does not change the generator, so one renderer's negotiation cannot affect
/// the next renderer's.
///
/// Cloning is cheap: what a generator holds is shared.
#[derive(Debug, Clone)]
pub struct A2uiGenerator {
    catalogs: Arc<[CatalogConfig]>,
    examples: Arc<[Vec<AgentMessage>]>,
    format: Arc<dyn OutputFormat>,
    accepts_inline_catalogs: bool,
    #[cfg(feature = "schema-validation")]
    schema_resources: Option<Arc<std::collections::BTreeMap<String, Value>>>,
}

impl A2uiGenerator {
    /// A generator for the catalogs an agent supports, asking its model for
    /// [`DirectJson`].
    pub fn new(catalogs: Vec<CatalogConfig>) -> Self {
        Self {
            catalogs: catalogs.into(),
            examples: Arc::new([]),
            format: Arc::new(DirectJson::new()),
            accepts_inline_catalogs: false,
            #[cfg(feature = "schema-validation")]
            schema_resources: None,
        }
    }

    /// Example turns to show the model, each a list of messages.
    ///
    /// Examples are checked against the active catalogs when a processor is
    /// created, not when an answer is parsed: that is where an agent author
    /// can act on a bad one.
    #[must_use]
    pub fn with_examples(mut self, examples: Vec<Vec<AgentMessage>>) -> Self {
        self.examples = examples.into();
        self
    }

    /// The format the agent's model is asked for.
    #[must_use]
    pub fn with_format(mut self, format: Arc<dyn OutputFormat>) -> Self {
        self.format = format;
        self
    }

    /// Whether catalogs a renderer declares inline in its capabilities become
    /// active. Off by default: an inline catalog is a schema the agent's
    /// author never saw.
    #[must_use]
    pub fn accepting_inline_catalogs(mut self, accepts: bool) -> Self {
        self.accepts_inline_catalogs = accepts;
        self
    }

    /// Also checks every message against the protocol's JSON Schemas and the
    /// active catalog's own, with [`SchemaValidator`]. See
    /// [`A2uiRequestProcessor::checking_schemas`].
    ///
    /// [`SchemaValidator`]: crate::schema_validation::SchemaValidator
    #[cfg(feature = "schema-validation")]
    #[must_use]
    pub fn checking_schemas(
        mut self,
        resources: std::collections::BTreeMap<String, Value>,
    ) -> Self {
        self.schema_resources = Some(Arc::new(resources));
        self
    }

    /// The catalogs the agent registered.
    pub fn catalogs(&self) -> &[CatalogConfig] {
        &self.catalogs
    }

    /// What this agent advertises to renderers: the ids of the catalogs it
    /// registered, in registration order, and whether it takes a renderer's
    /// inline catalogs.
    ///
    /// [`AgentCapabilities::to_json`] writes it as the protocol's capabilities
    /// object, and `adapters::a2a::agent_extension` takes the same two values
    /// for an A2A agent card.
    pub fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::supporting(
            self.catalogs
                .iter()
                .map(|config| config.transformed().catalog_id()),
        )
        .accepting_inline_catalogs(self.accepts_inline_catalogs)
    }

    /// The processor for a request from a renderer with these capabilities.
    ///
    /// The capabilities are an `Option` because
    /// [`RendererCapabilities::from_metadata`] returns one, and a request that
    /// carried none is refused here rather than served with a default: a
    /// processor is negotiated for one renderer. The reference SDK does the
    /// same (`A2uiGenerator.create_processor`: "if renderer_capabilities is
    /// None: raise A2uiCatalogError('Renderer capabilities are required to
    /// create a request processor.')"). An agent that wants to serve such a
    /// request with every catalog it registered calls [`resolve_catalogs`]
    /// with `None` and [`A2uiRequestProcessor::new`] itself.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when `capabilities` is `None` or leaves no
    /// catalog active, and what [`resolve_catalogs`] and
    /// [`A2uiRequestProcessor::new`] return.
    pub fn create_processor(
        &self,
        capabilities: Option<&RendererCapabilities>,
    ) -> Result<A2uiRequestProcessor> {
        self.create_processor_with(capabilities, self.format.clone())
    }

    /// [`A2uiGenerator::create_processor`], with a format that takes the place
    /// of the generator's own for this processor only.
    ///
    /// # Errors
    ///
    /// As [`A2uiGenerator::create_processor`].
    pub fn create_processor_with(
        &self,
        capabilities: Option<&RendererCapabilities>,
        format: Arc<dyn OutputFormat>,
    ) -> Result<A2uiRequestProcessor> {
        let Some(capabilities) = capabilities else {
            return Err(Error::catalog(
                "renderer capabilities are required to create a request processor",
            ));
        };
        let active = resolve_catalogs(
            &self.catalogs,
            Some(capabilities),
            self.accepts_inline_catalogs,
        )?;
        let processor = A2uiRequestProcessor::new(active, self.examples.to_vec(), format)?;
        #[cfg(feature = "schema-validation")]
        if let Some(resources) = &self.schema_resources {
            return processor.checking_schemas(resources);
        }
        Ok(processor)
    }
}

/// Everything one request needs: the catalogs negotiation left active, the
/// instructions for the model, and the reading of its answer.
///
/// Cloning is cheap: what a processor holds is shared, so a framework can
/// keep one beside the state it reads into, or move a copy into a task.
#[derive(Debug, Clone)]
pub struct A2uiRequestProcessor {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    documents: Arc<[CatalogDocument]>,
    /// How a state of this request takes a message in: the active catalogs
    /// registered, with their schemas when the session checks them. It holds
    /// no surfaces; every state starts as a copy of it.
    session: MessageProcessor,
    version: A2uiVersion,
    examples: Vec<Vec<AgentMessage>>,
    format: Arc<dyn OutputFormat>,
    prompt_snippet: String,
}

impl A2uiRequestProcessor {
    /// A processor for catalogs that are already active.
    ///
    /// [`A2uiGenerator::create_processor`] is the usual way to one; this is
    /// for a caller that resolved the catalogs itself.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when no catalog is active, when the catalogs
    /// target protocol versions that cannot share a session, and when the
    /// format cannot describe them. Returns [`Error::Validation`] when an
    /// example uses something the active catalogs do not declare.
    pub fn new(
        catalogs: Vec<CatalogDocument>,
        examples: Vec<Vec<AgentMessage>>,
        format: Arc<dyn OutputFormat>,
    ) -> Result<Self> {
        let version = check_catalogs(&catalogs)?;
        for turn in &examples {
            let messages = turn
                .iter()
                .map(serde_json::to_value)
                .collect::<std::result::Result<Vec<Value>, _>>()?;
            validate_payload(&catalogs, &messages)?;
        }
        let prompt_snippet = format.prompt_snippet(&catalogs, &examples)?;
        Ok(Self {
            inner: Arc::new(Inner {
                session: session_processor(&catalogs, version)?,
                documents: catalogs.into(),
                version,
                examples,
                format,
                prompt_snippet,
            }),
        })
    }

    /// A processor that also checks every message against the JSON Schemas:
    /// the protocol's envelope and common types as pinned in this crate, and
    /// each active catalog's own document.
    ///
    /// The semantic checks cannot see an undeclared property or a function
    /// argument of the wrong type; a schema engine can. `resources` supplies
    /// any other document a catalog refers to, keyed by absolute URI. Nothing
    /// is fetched.
    ///
    /// The schemas are those of the catalogs' protocol version; the v0.9
    /// family is held to the v0.9.1 documents, which take either of its
    /// discriminators.
    ///
    /// A schema is one catalog's. A v1.0 message in which a component or a
    /// function call names another catalog than its surface's own is not put
    /// to a schema: what it names is resolved against the active catalogs,
    /// and the properties and arguments drawn from the other catalog are not
    /// checked against that catalog's schema.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when a catalog document does not compile
    /// against the protocol's schemas: it refers to a document that is not
    /// among `resources`.
    #[cfg(feature = "schema-validation")]
    pub fn checking_schemas(
        &self,
        resources: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Self> {
        use crate::protocol::schema::SchemaBundle;
        use crate::protocol::schema_validation::SchemaValidator;

        let version = self.inner.version;
        let shown = if A2uiVersion::V0_9_1.accepts(version) {
            A2uiVersion::V0_9_1
        } else {
            version
        };
        let schemas = self
            .inner
            .documents
            .iter()
            .map(|document| {
                let bundle = SchemaBundle {
                    catalog: document.document().clone(),
                    ..SchemaBundle::for_version(shown)?
                };
                Ok((
                    document.catalog_id().to_string(),
                    SchemaValidator::new(&bundle, resources)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        self.with_schemas(schemas)
    }

    /// A processor that holds the messages of each named catalog's surfaces
    /// to the schemas compiled for it.
    #[cfg(feature = "schema-validation")]
    pub(crate) fn with_schemas(
        &self,
        schemas: Vec<(String, crate::protocol::schema_validation::SchemaValidator)>,
    ) -> Result<Self> {
        let mut session = self.inner.session.clone();
        for (catalog_id, schema) in schemas {
            session.register_schema(&catalog_id, schema)?;
        }
        Ok(Self {
            inner: Arc::new(Inner {
                documents: self.inner.documents.clone(),
                session,
                version: self.inner.version,
                examples: self.inner.examples.clone(),
                format: self.inner.format.clone(),
                prompt_snippet: self.inner.prompt_snippet.clone(),
            }),
        })
    }

    /// The catalogs negotiation left active, in the renderer's order of
    /// preference. They are the transformed ones.
    pub fn active_catalogs(&self) -> &[CatalogDocument] {
        &self.inner.documents
    }

    /// A router for the messages this request's renderer sends, over the
    /// active catalogs: it finds the agent function a `callAgentFunction` is
    /// for, and reads an `action`, an `error` and a
    /// `rendererFunctionResponse`. The functions it answers for are the ones
    /// the catalogs were given with
    /// [`CatalogDocument::with_agent_functions`].
    ///
    /// # Errors
    ///
    /// As [`RendererMessageRouter::new`], which the active catalogs of a
    /// processor do not give cause for.
    pub fn renderer_messages(&self) -> Result<RendererMessageRouter> {
        RendererMessageRouter::new(&self.inner.documents)
    }

    /// An empty table for the calls the agent makes to this request's
    /// renderer, which checks each call against the active catalogs
    /// ([`RendererCalls::with_catalogs`]). Keep it for the conversation.
    ///
    /// # Errors
    ///
    /// As [`RendererCalls::with_catalogs`], which the active catalogs of a
    /// processor do not give cause for.
    pub fn renderer_calls(&self) -> Result<RendererCalls> {
        RendererCalls::new().with_catalogs(&self.inner.documents)
    }

    /// The example turns the prompt shows.
    pub fn examples(&self) -> &[Vec<AgentMessage>] {
        &self.inner.examples
    }

    /// The format answers are read in.
    pub fn format(&self) -> &Arc<dyn OutputFormat> {
        &self.inner.format
    }

    /// The instructions for the model: how to write A2UI in the processor's
    /// format, for the active catalogs. Goes into the system prompt.
    pub fn prompt_snippet(&self) -> &str {
        &self.inner.prompt_snippet
    }

    /// An empty renderer state that holds the active catalogs.
    ///
    /// A chat agent makes one per turn, so every turn shows new UI. A canvas
    /// agent keeps one for the whole conversation and reads every turn into
    /// it, so each turn is checked against the surfaces earlier turns built.
    pub fn create_state(&self) -> RendererState {
        RendererState::from_parts(
            self.inner.documents.clone(),
            self.inner.version,
            &self.inner.session,
        )
    }

    /// Whether answers can be read into `state`: it holds the processor's
    /// active catalogs, in the same order, since the order decides which
    /// catalog an example or an assumed surface resolves to.
    ///
    /// A state made with [`A2uiRequestProcessor::create_state`] is always
    /// accepted. An agent that keeps a state per conversation calls this to
    /// tell whether the state still fits the request's catalogs, and makes a
    /// new one when it does not.
    pub fn accepts_state(&self, state: &RendererState) -> bool {
        let key = |catalog: &CatalogDocument| (catalog.catalog_id().to_string(), catalog.version());
        state
            .catalogs()
            .map(key)
            .eq(self.inner.documents.iter().map(key))
    }

    /// Reads a finished answer as one chat turn: against a renderer that
    /// holds no surfaces yet.
    ///
    /// # Errors
    ///
    /// As [`A2uiRequestProcessor::parse_response_in`].
    pub fn parse_response(
        &self,
        answer: &str,
    ) -> std::result::Result<Vec<ResponsePart>, ResponseError> {
        self.parse_response_in(answer, &mut self.create_state())
    }

    /// Reads a finished answer into `state`: splits it into text and blocks,
    /// compiles each block, and checks and applies every message.
    ///
    /// Nothing has been sent when a whole answer is read, so it is checked as
    /// a whole: a component may name a child that a later block adds, and the
    /// trees of the surfaces the answer touched are checked once every block
    /// is applied.
    ///
    /// **All or nothing.** If the answer is rejected, `state` is left exactly
    /// as it was. The messages returned on success have all been applied, in
    /// order, and are what the agent sends.
    ///
    /// # Errors
    ///
    /// The error is [`Error::Parse`] when a block cannot be read,
    /// [`Error::Validation`] when the state rejects a message or a tree, with
    /// every failure located in the message that has to change, and
    /// [`Error::Catalog`] when `state` holds other catalogs than the
    /// processor. Its `parts` hold what was read before the failing block, or
    /// only the text when a tree check fails, since that check spans every
    /// block. None of them was applied.
    pub fn parse_response_in(
        &self,
        answer: &str,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<ResponsePart>, ResponseError> {
        self.read(self.inner.format.unwrap(answer), state)
    }

    /// Reads `block` as the body of one block, without tags, into `state`: a
    /// tool call argument, say, where the framework already knows the whole
    /// text is A2UI. Otherwise as
    /// [`A2uiRequestProcessor::parse_response_in`], all or nothing included.
    ///
    /// # Errors
    ///
    /// As [`A2uiRequestProcessor::parse_response_in`].
    pub fn parse_block_in(
        &self,
        block: &str,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<ResponsePart>, ResponseError> {
        self.read(vec![RawPart::block(block)], state)
    }

    /// Opens a stream that reads one answer into `state` chunk by chunk.
    ///
    /// The stream keeps a copy of `state` as it is now, for
    /// [`ResponseStream::rewind`]. Pass the same state to every call on the
    /// stream.
    ///
    /// Unlike an answer read whole, a streamed one is not all or nothing: see
    /// [`ResponseStream`] for what a failure leaves in the state.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Catalog`] when `state` holds other catalogs than the
    /// processor, and [`Error::Parse`] when the processor's format can only be
    /// read whole.
    pub fn open_stream(
        &self,
        state: &RendererState,
        options: StreamOptions,
    ) -> Result<ResponseStream> {
        self.check_state(state)?;
        let reader = self
            .inner
            .format
            .reader_for(self.inner.version, options.wrapped)
            .ok_or_else(|| {
                Error::parse(format!(
                    "the {} format cannot be read chunk by chunk",
                    self.inner.format.name()
                ))
            })?;
        let mut opened = state.clone();
        opened.bind(&self.inner.session);
        Ok(ResponseStream::new(
            reader,
            opened,
            options.after_rejected_block,
        ))
    }

    fn check_state(&self, state: &RendererState) -> Result<()> {
        if self.accepts_state(state) {
            return Ok(());
        }
        fn named<'a>(catalogs: impl Iterator<Item = &'a CatalogDocument>) -> Vec<String> {
            catalogs
                .map(|catalog| format!("{} ({})", catalog.catalog_id(), catalog.version()))
                .collect()
        }
        Err(Error::catalog(format!(
            "the renderer state holds catalogs {:?}, but the processor's active catalogs are {:?}",
            named(state.catalogs()),
            named(self.inner.documents.iter()),
        )))
    }

    fn read(
        &self,
        raw_parts: Vec<RawPart>,
        state: &mut RendererState,
    ) -> std::result::Result<Vec<ResponsePart>, ResponseError> {
        self.check_state(state).map_err(ResponseError::new)?;
        state.bind(&self.inner.session);

        let mut parts: Vec<ResponsePart> = Vec::new();
        let mut touched: Vec<String> = Vec::new();
        // Surfaces touched by a block the answer ended inside. A cut-off block
        // is missing what it would have added, so their trees are not checked.
        let mut incomplete: Vec<String> = Vec::new();
        let read = state.transaction(|state| {
            state.begin_answer();
            let mut index = 0;
            for raw in raw_parts {
                let (block, closed) = match raw {
                    RawPart::Text(text) => {
                        parts.push(ResponsePart::Text(text));
                        continue;
                    }
                    RawPart::Block { raw, closed } => (raw, closed),
                };
                let mut messages = Vec::new();
                let compiled = self
                    .inner
                    .format
                    .compile(&block, closed)
                    .map_err(|error| (error, false))?;
                for message in &compiled {
                    let applied = state
                        .apply_at(index, message)
                        .map_err(|error| (error, false))?;
                    index += 1;
                    if !touched.contains(&applied.surface_id) {
                        touched.push(applied.surface_id.clone());
                    }
                    if !closed && !incomplete.contains(&applied.surface_id) {
                        incomplete.push(applied.surface_id);
                    }
                    messages.push(applied.message);
                }
                parts.push(ResponsePart::A2ui(messages));
            }
            state
                .check_trees(touched.iter().filter(|id| !incomplete.contains(*id)))
                .map_err(|error| (error, true))
        });
        match read {
            Ok(()) => Ok(parts),
            // The tree check spans every block, so no block is known to be
            // good: only the text is kept.
            Err((error, true)) => Err(ResponseError {
                error,
                parts: parts
                    .into_iter()
                    .filter(|part| matches!(part, ResponsePart::Text(_)))
                    .collect(),
            }),
            Err((error, false)) => Err(ResponseError { error, parts }),
        }
    }
}
