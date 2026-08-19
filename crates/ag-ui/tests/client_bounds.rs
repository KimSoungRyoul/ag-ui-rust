//! `Session` and its friends are nameable without knowing the transport.
//!
//! A bound on a struct definition is viral: put `T: Transport` on `Session<T, S>`
//! and every application helper that so much as mentions the type in a signature
//! has to repeat it, including ones that only read `messages()`. The bound
//! belongs on the impl blocks that actually call transport methods, and this
//! file is what says so — it is written the way an application writes helpers,
//! and it fails to *compile* if the bound migrates back onto the type.

#![cfg(feature = "client")]

use ag_ui::client::transport::ReplayTransport;
use ag_ui::client::{RunStream, Session, SessionBuilder};

fn count<T, S>(session: &Session<T, S>) -> usize {
    session.messages().len()
}

fn seed<T, S>(builder: SessionBuilder<T, S>) -> SessionBuilder<T, S> {
    builder.verify(false)
}

fn describe<T, S>(run: &RunStream<'_, T, S>) -> String {
    format!("{run:?}")
}

/// An application holding a session in its own state, deriving `Debug`.
#[derive(Debug)]
struct App<T, S> {
    session: Session<T, S>,
}

#[test]
fn helpers_naming_a_session_need_no_transport_bound() {
    let session: Session<ReplayTransport> = Session::new(ReplayTransport::new([]), "thread-1");
    assert_eq!(count(&session), 0);

    let builder: SessionBuilder<ReplayTransport> =
        Session::builder(ReplayTransport::new([]), "thread-1");
    assert_eq!(count(&seed(builder).build()), 0);

    let mut app = App { session };
    assert!(format!("{app:?}").contains("Session"));
    assert!(describe(&app.session.run()).contains("RunStream"));
}
