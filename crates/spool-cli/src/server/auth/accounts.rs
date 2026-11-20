//! Work on the accounts, done off the threads that answer requests, and
//! no more of it at once than hashes a password.

use std::sync::Arc;

use spool::ActorNames;
use spool::core::Actor;
use tokio::sync::OwnedSemaphorePermit;

use crate::auth::{Accounts, Refused};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};

/// Runs work on the accounts on a blocking thread: hashing a password takes
/// a while.
pub(super) async fn run_accounts_work<T: Send + 'static>(
    app: &App,
    work: impl FnOnce(&Accounts) -> Result<T, Refused> + Send + 'static,
) -> ApiResult<T> {
    let accounts = app.accounts.clone();
    match tokio::task::spawn_blocking(move || work(&accounts)).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(ApiError::internal("the accounts stopped while working")),
    }
}

/// What each person is called, by the actor the log names them as: a list
/// sorted by who did something sorts them as the studio shows them.
pub(in crate::server) async fn list_actor_names(app: &App) -> ApiResult<ActorNames> {
    let people = run_accounts_work(app, Accounts::list).await?;
    Ok(people
        .into_iter()
        .map(|person| (Actor::User(person.id), person.name))
        .collect())
}

/// Runs work that hashes a password, which `permit` allows: it is held until
/// the work is done, even if whoever asked has gone.
pub(super) async fn run_hashing_work<T: Send + 'static>(
    app: &App,
    permit: OwnedSemaphorePermit,
    work: impl FnOnce(&Accounts) -> Result<T, Refused> + Send + 'static,
) -> ApiResult<T> {
    run_accounts_work(app, move |accounts| {
        let _held = permit;
        work(accounts)
    })
    .await
}

/// Leave to hash a password, for a caller the server has reason to wait
/// for: one who is signed in, or who has the setup code.
pub(super) async fn acquire_hashing_permit(app: &App) -> ApiResult<OwnedSemaphorePermit> {
    Arc::clone(&app.hashing)
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("the accounts stopped while working"))
}
