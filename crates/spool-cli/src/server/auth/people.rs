//! The people an admin manages: the handlers of `/v1/people`.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};

use super::accounts::{acquire_hashing_permit, run_accounts_work, run_hashing_work};
use super::caller::Caller;
use crate::api::{AddPersonRequest, ChangePersonRequest, PersonList};
use crate::auth::{Accounts, Change, New};
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::request::json_body;
use crate::server::work::ok_json;

pub(in crate::server) async fn list_people(State(app): State<App>) -> ApiResult {
    ok_json(PersonList {
        people: run_accounts_work(&app, Accounts::list).await?,
    })
}

pub(in crate::server) async fn add_person(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    caller.admin()?;
    let request: AddPersonRequest = json_body(&body)?;
    let sets_password = request.password.is_some();
    let add = move |accounts: &Accounts| {
        accounts.add(New {
            username: request.username,
            name: request.name,
            password: request.password,
            email: request.email,
            admin: request.admin.unwrap_or(false),
        })
    };
    ok_json(match sets_password {
        true => run_hashing_work(&app, acquire_hashing_permit(&app).await?, add).await?,
        false => run_accounts_work(&app, add).await?,
    })
}

pub(in crate::server) async fn change_person(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(person): Path<String>,
    body: Bytes,
) -> ApiResult {
    caller.admin()?;
    let request: ChangePersonRequest = json_body(&body)?;
    let change = Change {
        name: request.name,
        password: request.password,
        email: request.email,
        admin: request.admin,
        active: request.active,
    };
    let sets_password = change.password.is_some();
    let changed = move |accounts: &Accounts| accounts.change(&person, change);
    ok_json(match sets_password {
        true => run_hashing_work(&app, acquire_hashing_permit(&app).await?, changed).await?,
        false => run_accounts_work(&app, changed).await?,
    })
}
