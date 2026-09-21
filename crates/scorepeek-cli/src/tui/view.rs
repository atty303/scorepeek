use scorepeek_frontend_api::{ApplicationSnapshot, RunSnapshot};

#[must_use]
pub fn active_run(snapshot: &ApplicationSnapshot) -> Option<&RunSnapshot> {
    snapshot.run.as_ref()
}
