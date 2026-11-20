//! The tokens an admin makes and revokes: the handlers of `/v1/tokens`.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};

use super::accounts::run_accounts_work;
use super::caller::Caller;
use crate::api::{AddTokenRequest, AddedToken, TokenList};
use crate::auth::Accounts;
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::request::json_body;
use crate::server::work::ok_json;

pub(in crate::server) async fn list_tokens(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
) -> ApiResult {
    caller.admin()?;
    ok_json(TokenList {
        tokens: run_accounts_work(&app, Accounts::tokens).await?,
    })
}

pub(in crate::server) async fn add_token(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let by = caller.admin()?.id.clone();
    let request: AddTokenRequest = json_body(&body)?;
    let (token, secret) = run_accounts_work(&app, move |accounts| {
        accounts.add_token(&request.name, Some(&by))
    })
    .await?;
    ok_json(AddedToken { token, secret })
}

pub(in crate::server) async fn revoke_token(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(token): Path<String>,
) -> ApiResult {
    caller.admin()?;
    ok_json(run_accounts_work(&app, move |accounts| accounts.revoke_token(&token)).await?)
}
