//! Who may call the API: the people with an account, their sessions, the
//! tokens of callers that are not people, and the assistants let in with
//! OAuth.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Someone with an account on this server. The log names a person by `id`,
/// as `user:<id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Person {
    pub id: String,
    pub username: String,
    pub name: String,
    /// May add people and change their accounts.
    pub admin: bool,
    /// May sign in. An account that is turned off keeps its name in the log.
    pub active: bool,
    /// The address of the Google account they sign in with, where the
    /// server signs people in with Google. `null` for someone who signs in
    /// with a password only.
    pub email: Option<String>,
    /// Has a password to sign in with. Someone who has none signs in with
    /// Google.
    pub password: bool,
}

/// `GET /auth/session`: who the caller is signed in as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionState {
    pub person: Option<Person>,
    /// No admin can sign in yet. The server printed a link when it started
    /// that makes the first account.
    pub setup: bool,
    /// A development server: nobody signs in, and whoever opens it is the
    /// `dev` account.
    pub dev: bool,
    /// The server signs people in with Google: `GET /auth/google` in a
    /// browser starts it.
    pub google: bool,
}

/// `POST /auth/sign-in`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignInRequest {
    pub username: String,
    pub password: String,
}

/// `POST /auth/setup`: make the first account, an admin, with the code from
/// the link the server printed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    pub code: String,
    pub username: String,
    pub name: String,
    pub password: String,
}

/// `POST /auth/password`: change the caller's own password.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordRequest {
    pub current: String,
    pub new: String,
}

/// `POST /v1/people`: add a person. Only an admin may. They sign in with
/// the password, with Google by the email, or with either: one of the two is
/// needed, and the email alone only where the server signs people in with
/// Google.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPersonRequest {
    pub username: String,
    pub name: String,
    #[serde(default)]
    pub password: Option<String>,
    /// The address of the Google account they sign in with.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub admin: Option<bool>,
}

/// `POST /v1/people/{id}`: change a person. Only an admin may. A field left
/// out stays as it is. A new password, or turning the account off, signs
/// the person out everywhere. Another email signs them out wherever they
/// signed in with Google.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangePersonRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// The address of the Google account they sign in with. An empty one
    /// takes it away, for someone who has a password.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub admin: Option<bool>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// `GET /v1/people`: everyone with an account, in the order they were added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonList {
    pub people: Vec<Person>,
}

/// A token: what a caller that is not a person presents, as
/// `Authorization: Bearer <token>`. The log names what it changed by the
/// token's name, as `agent:<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Token {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// The ID of the person who made it. `null` when it was made on the
    /// server's machine with `spool token add`.
    pub created_by: Option<String>,
    /// When it was last presented, to the minute. `null` if it never was.
    pub last_used_at: Option<String>,
}

/// `POST /v1/tokens`: make a token. Only an admin may.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTokenRequest {
    /// What its changes are recorded under, as `agent:<name>`: 1 to 64
    /// letters, digits, `.`, `_` or `-`. No other token has it.
    pub name: String,
}

/// A token that was just made. This is the only time the server says what
/// the token is: it keeps a hash of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddedToken {
    pub token: Token,
    /// What the caller presents.
    pub secret: String,
}

/// `GET /v1/tokens`: every token, in the order they were made. Only an
/// admin may read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenList {
    pub tokens: Vec<Token>,
}

/// An assistant that a person let in with OAuth, such as Claude or ChatGPT
/// on the web. It works at `/mcp` as that person, and what it changes is
/// recorded under them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Grant {
    pub id: String,
    /// The ID of the person who approved it.
    pub person: String,
    /// What the assistant calls itself. Nobody checked it.
    pub client_name: Option<String>,
    /// Where its approval was sent, which says whose it is: such as
    /// `https://claude.ai/api/mcp/auth_callback`.
    pub redirect_uri: String,
    pub created_at: String,
    /// When it last called `/mcp`, to the minute. `null` if it never did.
    pub last_used_at: Option<String>,
}

/// `GET /v1/grants`: the assistants people let in, in the order they were
/// approved. Only an admin may read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GrantList {
    pub grants: Vec<Grant>,
}

/// `GET /auth/oauth?<request>`: what an assistant that sent someone to
/// `/oauth/authorize` asks for, to show them before they approve it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OAuthRequest {
    /// What the assistant calls itself. Nobody checked it.
    pub client_name: Option<String>,
    /// Where the approval is sent.
    pub redirect_uri: String,
    /// The host of `redirect_uri`, which is who gets the approval.
    pub host: String,
    /// Whether the approval goes to a program on the approver's own
    /// machine, which any program there could be.
    pub loopback: bool,
}

/// `POST /auth/oauth`: approve an assistant, or turn it away.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OAuthDecision {
    /// The query of the `/oauth/authorize` address the assistant sent.
    pub request: String,
    pub approve: bool,
}

/// Where to send the browser with the decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OAuthDecided {
    pub to: String,
}
