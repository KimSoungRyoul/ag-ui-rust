//! Authoring one surface with a model: a fixed target, version and catalog,
//! and a surface that is only ever handed back validated.
//!
//! [`A2uiAuthor`] is a use of the [agent layer](crate::agent) for the case
//! where an application wants one surface created or edited and nothing else:
//! it asks the layer for the prompt, reads the model's answer with the layer's
//! parser into a renderer state, and lets the layer's [`RetryDriver`] say when
//! to ask again. What it adds is the request's own contract: which surface,
//! which version, which catalog, and whether the request creates or edits.
//! A framework that streams, serves several surfaces or negotiates catalogs
//! per renderer uses the agent layer directly.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use serde_json::Value;

use crate::agent::{
    A2uiRequestProcessor, CatalogDocument, DirectJson, RendererState, ResponsePart, RetryDriver,
    Verdict,
};
use crate::protocol::schema::SchemaBundle;
use crate::protocol::schema_validation::SchemaValidator;
use crate::protocol::surface::SurfaceState;
use crate::{
    A2uiVersion, AgentMessage, AgentPayload, DataModel, Error, ErrorCode, Result, ValidationError,
};

#[derive(Debug)]
struct Contract {
    version: A2uiVersion,
    catalog_id: String,
    /// The agent layer's processor for the one catalog, checking the schemas
    /// the author was built with.
    processor: A2uiRequestProcessor,
}

/// Shared immutable configuration for independently generated surfaces.
#[derive(Debug, Clone)]
pub struct A2uiAuthor {
    contract: Arc<Contract>,
    max_attempts: u32,
    send_data_model: Option<bool>,
}

/// A validated batch and its resulting state. Fields cannot be mutated or
/// deserialized to manufacture successful validation. Revalidate stored history.
#[derive(Debug, Clone)]
pub struct ValidatedSurface {
    operations: Vec<AgentMessage>,
    history: Vec<AgentMessage>,
    state: SurfaceState,
    attempts: u32,
}
impl ValidatedSurface {
    /// Operations in this batch, for transmission exactly once by the caller.
    pub fn operations(&self) -> &[AgentMessage] {
        &self.operations
    }
    /// Full observed operation history, for storage and subsequent revalidation.
    pub fn history(&self) -> &[AgentMessage] {
        &self.history
    }
    /// Renderer-scoped surface ID.
    pub fn surface_id(&self) -> &str {
        &self.state.surface_id
    }
    /// Catalog fixed for the life of this surface.
    pub fn catalog_id(&self) -> &str {
        self.state.catalog_id.as_deref().unwrap_or_default()
    }
    /// Resulting components, including prior definitions for an edit.
    pub fn components(&self) -> &[crate::Component] {
        &self.state.components
    }
    /// Resulting exact local data model.
    pub fn data_model(&self) -> &DataModel {
        &self.state.data_model
    }
    /// One-based count of generation attempts (zero for manual validation).
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

/// Whether a batch creates its surface or edits one that exists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Request {
    Create,
    Edit,
}

impl A2uiAuthor {
    /// An author on the official schemas and basic catalog of `version`, as
    /// pinned in this crate ([`SchemaBundle::for_version`]).
    pub fn basic(version: A2uiVersion) -> Result<Self> {
        Self::new(
            version,
            SchemaBundle::for_version(version)?,
            BTreeMap::new(),
        )
    }
    /// Creates a custom author from complete schemas and explicit local references.
    ///
    /// Every message is held to the bundle's schemas, compiled once here. The
    /// prompt shows the bundle's catalog, with the protocol's own schemas as
    /// this crate pins them.
    pub fn new(
        version: A2uiVersion,
        bundle: SchemaBundle,
        resources: BTreeMap<String, Value>,
    ) -> Result<Self> {
        let schema = SchemaValidator::new(&bundle, &resources)?;
        let catalog = CatalogDocument::load(bundle.catalog, None, Some(version))?;
        let catalog_id = catalog.catalog_id().to_string();
        let processor =
            A2uiRequestProcessor::new(vec![catalog], Vec::new(), Arc::new(DirectJson::new()))?
                .with_schemas(vec![(catalog_id.clone(), schema)])?;
        Ok(Self {
            contract: Arc::new(Contract {
                version,
                catalog_id,
                processor,
            }),
            max_attempts: crate::constants::MAX_A2UI_ATTEMPTS,
            send_data_model: None,
        })
    }
    /// Limits generated-document correction attempts; provider errors are never retried.
    #[must_use]
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }
    /// Requires a `sendDataModel` setting for generated and restored surfaces.
    /// `true` requires an explicit flag; `false` also accepts omission, the protocol
    /// default. Without this policy, either setting is accepted. Edits revalidate
    /// the original creation flag and cannot change it.
    #[must_use]
    pub fn with_send_data_model(mut self, enabled: bool) -> Self {
        self.send_data_model = Some(enabled);
        self
    }
    /// The agent layer's processor this author reads answers with: its prompt
    /// snippet, its active catalog, its format.
    pub fn processor(&self) -> &A2uiRequestProcessor {
        &self.contract.processor
    }
    /// Requests a new surface with a fixed ID, version, and catalog.
    pub fn create<'a>(
        &'a self,
        id: impl Into<String>,
        request: impl Into<String>,
    ) -> AuthorRequest<'a> {
        AuthorRequest {
            author: self,
            id: id.into(),
            request: request.into(),
            prior: None,
        }
    }
    /// Requests updates to a previously validated surface. Creation, deletion,
    /// cross-surface updates, and version/catalog changes are rejected.
    pub fn edit<'a>(
        &'a self,
        prior: &'a ValidatedSurface,
        request: impl Into<String>,
    ) -> AuthorRequest<'a> {
        AuthorRequest {
            author: self,
            id: prior.surface_id().into(),
            request: request.into(),
            prior: Some(prior),
        }
    }
    /// Validates stored or manually authored raw messages before any typed decoding.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] with every failure located in the message
    /// that has to change, whether a renderer would reject the message or it
    /// breaks the request's own contract (another surface, another version,
    /// an operation a create may not carry).
    pub fn validate_create(&self, id: &str, messages: &[Value]) -> Result<ValidatedSurface> {
        self.validate(id, messages, None)
    }
    /// Validates a manually authored edit using the complete prior state.
    ///
    /// # Errors
    ///
    /// As [`A2uiAuthor::validate_create`], and [`Error::Catalog`] when `prior`
    /// was made under another catalog or version.
    pub fn validate_edit(
        &self,
        prior: &ValidatedSurface,
        messages: &[Value],
    ) -> Result<ValidatedSurface> {
        self.validate(prior.surface_id(), messages, Some(prior))
    }

    fn validate(
        &self,
        id: &str,
        messages: &[Value],
        prior: Option<&ValidatedSurface>,
    ) -> Result<ValidatedSurface> {
        let mut state = self.starting_state(id, prior)?;
        state.apply(messages)?;
        // Every message was applied, so every message decodes.
        let operations = messages
            .iter()
            .map(|message| serde_json::from_value(message.clone()))
            .collect::<std::result::Result<Vec<AgentMessage>, _>>()?;
        self.accept(id, operations, prior, &state)
    }

    /// The renderer state a request starts from: empty for a create, and the
    /// prior surface replayed for an edit.
    ///
    /// A matching catalog ID does not prove that two author instances compiled
    /// the same contract, so an edit replays the retained wire history under
    /// this author's contract before it can hide an incompatible old
    /// definition by replacing it.
    fn starting_state(&self, id: &str, prior: Option<&ValidatedSurface>) -> Result<RendererState> {
        if id.is_empty() {
            return Err(Error::parse("a target surface ID is required"));
        }
        let mut state = self.contract.processor.create_state();
        let Some(prior) = prior else {
            return Ok(state);
        };
        if prior.catalog_id() != self.contract.catalog_id
            || prior.state.version != self.contract.version.as_str()
        {
            return Err(Error::catalog(
                "prior surface uses a different catalog or version",
            ));
        }
        let history = prior
            .history
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        state.apply(&history)?;
        self.held_to_the_request(id, &prior.history, Request::Create, &state)?;
        Ok(state)
    }

    /// The surface a batch leaves, once the batch is held to the request.
    fn accept(
        &self,
        id: &str,
        operations: Vec<AgentMessage>,
        prior: Option<&ValidatedSurface>,
        state: &RendererState,
    ) -> Result<ValidatedSurface> {
        let request = if prior.is_some() {
            Request::Edit
        } else {
            Request::Create
        };
        self.held_to_the_request(id, &operations, request, state)?;
        let surface = state
            .surface(id)
            .cloned()
            .ok_or_else(|| Error::parse("createSurface is required"))?;
        let mut history = prior.map_or_else(Vec::new, |prior| prior.history.clone());
        history.extend(operations.iter().cloned());
        Ok(ValidatedSurface {
            operations,
            history,
            state: surface,
            attempts: 0,
        })
    }

    /// Holds a batch the renderer state accepted to what the request asked
    /// for, and the surface it leaves to a whole tree.
    fn held_to_the_request(
        &self,
        id: &str,
        operations: &[AgentMessage],
        request: Request,
        state: &RendererState,
    ) -> Result<()> {
        let faults = self.contract_faults(id, operations, request);
        if !faults.is_empty() {
            return Err(Error::Validation {
                errors: faults.into(),
            });
        }
        state.check_trees([id])
    }

    /// What a batch breaks of the request's own contract: one target surface,
    /// one version, a create that creates and an edit that only updates.
    /// Each is located in the message that has to change.
    fn contract_faults(
        &self,
        id: &str,
        operations: &[AgentMessage],
        request: Request,
    ) -> Vec<ValidationError> {
        let invalid = |path: String, reason: String| {
            ValidationError::new(ErrorCode::InvalidValue, path, reason)
        };
        if operations.is_empty() {
            return vec![invalid(
                "response".to_string(),
                "The answer carries no A2UI message. Return the messages for the surface."
                    .to_string(),
            )];
        }
        let version = self.contract.version.as_str();
        let mut faults = Vec::new();
        let mut draws = false;
        for (index, operation) in operations.iter().enumerate() {
            let at = format!("messages[{index}]");
            let key = operation
                .payload
                .operation()
                .map_or("message", |operation| operation.as_str());
            if operation.version != version {
                faults.push(invalid(
                    format!("{at}.version"),
                    format!(
                        "The request is for version {version}; this message states {}.",
                        operation.version
                    ),
                ));
            }
            if operation.surface_id() != Some(id) {
                faults.push(invalid(
                    format!("{at}.{key}.surfaceId"),
                    format!(
                        "The request is for surface {id:?}; this message names {:?}.",
                        operation.surface_id().unwrap_or_default()
                    ),
                ));
            }
            match (&operation.payload, request) {
                (AgentPayload::CreateSurface(create), Request::Create) => {
                    if let Some(expected) = self.send_data_model {
                        if create.send_data_model.unwrap_or(false) != expected {
                            let requirement = if expected { "true" } else { "false or omitted" };
                            faults.push(invalid(
                                format!("{at}.createSurface.sendDataModel"),
                                format!("sendDataModel must be {requirement} for this author"),
                            ));
                        }
                    }
                }
                (AgentPayload::UpdateComponents(_), _) => draws = true,
                (AgentPayload::UpdateDataModel(_), _) => {}
                (_, Request::Create) => faults.push(invalid(
                    at,
                    format!(
                        "A request to create a surface takes one createSurface followed by \
                         updates; {key} is not one of them."
                    ),
                )),
                (_, Request::Edit) => faults.push(invalid(
                    at,
                    format!(
                        "A request to edit a surface takes updateComponents and \
                         updateDataModel only; {key} is not one of them."
                    ),
                )),
            }
        }
        if request == Request::Create && !draws && faults.is_empty() {
            faults.push(ValidationError::new(
                ErrorCode::EmptyComponents,
                "response",
                format!(
                    "Surface {id:?} is created with nothing in it. Follow createSurface with an \
                     updateComponents that draws it."
                ),
            ));
        }
        faults
    }
}

/// A request whose model callback is awaited without imposing an executor or Send bound.
#[derive(Debug)]
pub struct AuthorRequest<'a> {
    author: &'a A2uiAuthor,
    id: String,
    request: String,
    prior: Option<&'a ValidatedSurface>,
}
impl AuthorRequest<'_> {
    /// The prompt for the model: the request's target, then the agent layer's
    /// instructions for the format and the catalog the answer is validated
    /// against, then what an edit starts from.
    pub fn prompt(&self) -> String {
        let contract = &self.author.contract;
        let intent = if self.prior.is_some() {
            "Edit the existing surface using updateComponents/updateDataModel only."
        } else {
            "Create exactly one surface, followed by component/data updates."
        };
        let mut prompt = format!(
            "{intent}\nTarget surfaceId: {}\nExact version: {}\nExact catalogId: {}\nRequest: {}\n\n{}",
            self.id,
            contract.version.as_str(),
            contract.catalog_id,
            self.request,
            contract.processor.prompt_snippet()
        );
        if self.prior.is_none() {
            if let Some(enabled) = self.author.send_data_model {
                prompt.push_str(&format!(
                    "\nRequired createSurface.sendDataModel: {enabled}."
                ));
                if !enabled {
                    prompt.push_str(" Omitting sendDataModel also means false.");
                }
            }
        }
        if let Some(prior) = self.prior {
            prompt.push_str(&format!("\nObserved prior operations (local observation, may be stale):\n{}\nUpdate only requested data paths; preserve unrelated user input.",serde_json::to_string(prior.history()).expect("validated operations serialize")));
        }
        prompt
    }
    /// Generates and corrects invalid documents. Provider errors propagate immediately
    /// with their source. Send callbacks produce Send futures; local callbacks work too.
    ///
    /// `generate` is handed the prompt and the one-based number of the
    /// attempt. Its answer is the messages as a JSON array, inside
    /// `<a2ui-json>` tags or on its own. An answer that is rejected is asked
    /// for again, with each failure located where it is in the answer.
    ///
    /// # Errors
    ///
    /// Returns what `generate` returns, [`Error::RecoveryExhausted`] with the
    /// last attempt's failures when every attempt is rejected, and what
    /// [`A2uiAuthor::validate_edit`] returns for a prior surface this author
    /// cannot edit, before the model is called.
    pub async fn generate<F, Fut>(self, mut generate: F) -> Result<ValidatedSurface>
    where
        F: FnMut(String, u32) -> Fut,
        Fut: Future<Output = Result<String>>,
    {
        let author = self.author;
        let start = author.starting_state(&self.id, self.prior)?;
        let processor = &author.contract.processor;
        let original = self.prompt();
        let mut prompt = original.clone();
        let mut retry = RetryDriver::new(processor, author.max_attempts);
        loop {
            let answer = generate(prompt, retry.attempts() + 1).await?;
            // Each attempt is read into a copy, so one that is turned down
            // for the request's own reasons leaves nothing behind.
            let mut state = start.clone();
            let verdict = if processor.format().has_content(&answer, false) {
                retry.judge(&answer, &mut state)
            } else {
                retry.judge_block(&answer, &mut state)
            };
            let feedback = match verdict {
                Verdict::Done(parts) => {
                    let operations: Vec<AgentMessage> = parts
                        .into_iter()
                        .flat_map(|part| match part {
                            ResponsePart::A2ui(messages) => messages,
                            ResponsePart::Text(_) => Vec::new(),
                        })
                        .collect();
                    match author.accept(&self.id, operations, self.prior, &state) {
                        Ok(mut surface) => {
                            surface.attempts = retry.attempts();
                            return Ok(surface);
                        }
                        // Valid A2UI, and not what the request asked for.
                        Err(Error::Validation { errors }) => retry.reject(errors.0)?,
                        Err(other) => retry.reject(vec![ValidationError::new(
                            ErrorCode::InvalidValue,
                            "response",
                            other.to_string(),
                        )])?,
                    }
                }
                Verdict::Retry(feedback) => feedback,
                Verdict::GiveUp(error) => return Err(error),
            };
            prompt = feedback.prompt(&original);
        }
    }
}
