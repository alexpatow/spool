//! Sign-in with Google: what the server asks Google, and what it takes from
//! the answer.
//!
//! Google is asked who someone is, with OpenID Connect: the browser is sent
//! to Google, comes back with a code, and the code is exchanged for a token
//! Google signed, which names the account and its address. The exchange and
//! the checks of the token are `openidconnect`'s. Whether that account may
//! use this server is not Google's to say: see
//! [`Accounts::enter_with_google`](crate::auth::Accounts::enter_with_google).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
    CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreRevocableToken, CoreRevocationErrorResponse,
    CoreTokenIntrospectionResponse, CoreTokenType,
};
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, IdTokenFields,
    IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RequestTokenError, Scope,
    StandardErrorResponse, StandardTokenResponse, TokenResponse,
};
use serde::{Deserialize, Serialize};
use ureq::http::{Method, Response, header::CONTENT_TYPE};

/// Where Google says what its sign-in is.
pub const ISSUER: &str = "https://accounts.google.com";

/// The path Google sends a browser back to.
pub const CALLBACK: &str = "/auth/google/callback";

/// How long to wait for an answer from Google.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long what Google says of its sign-in is kept before it is asked
/// again: where its pages are, and the keys it signs with, which it changes.
const KEPT_FOR: Duration = Duration::from_secs(60 * 60);

/// The largest answer read. Google's are a few thousand bytes.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;

/// The client this server is to Google: made in Google's console, with
/// [`Google::redirect`] as its redirect URI.
#[derive(Clone)]
pub struct Google {
    issuer: IssuerUrl,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect: RedirectUrl,
}

impl std::fmt::Debug for Google {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Without the secret: options are printed.
        f.debug_struct("Google")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id.as_str())
            .field("redirect", &self.redirect.as_str())
            .finish_non_exhaustive()
    }
}

impl Google {
    /// `address` is where people reach this server, such as
    /// `https://spool.example.com`. `issuer` is [`ISSUER`], or what stands
    /// in for Google in a test.
    pub fn new(
        issuer: &str,
        client_id: &str,
        client_secret: &str,
        address: &str,
    ) -> Result<Self, String> {
        let address = address.trim_end_matches('/');
        Ok(Self {
            issuer: IssuerUrl::new(issuer.to_owned())
                .map_err(|error| format!("`{issuer}` is not a URL: {error}"))?,
            client_id: ClientId::new(client_id.to_owned()),
            client_secret: ClientSecret::new(client_secret.to_owned()),
            redirect: RedirectUrl::new(format!("{address}{CALLBACK}"))
                .map_err(|error| format!("`{address}` is not a URL: {error}"))?,
        })
    }

    /// The address Google sends a browser back to, which Google's console
    /// calls an authorized redirect URI.
    pub fn redirect(&self) -> &str {
        self.redirect.as_str()
    }
}

/// A sign-in that has been sent to Google.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    /// Where to send the browser.
    pub url: String,
    /// What Google sends back with the browser, to tell that it is the
    /// browser that was sent.
    pub state: String,
    /// What the token Google signs repeats, to tell that it was made for
    /// this sign-in.
    pub nonce: String,
    /// What proves to Google that whoever brings the code is who asked for
    /// it.
    pub verifier: String,
}

/// Who Google says someone is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// What Google calls the account. It stays the same whatever becomes of
    /// the address.
    pub subject: String,
    /// The account's address, if it is one Google answers for: an address
    /// of Gmail, or of an organisation's Google Workspace. Then whoever has
    /// the Google account has the address.
    ///
    /// A Google account can be made with an address of any other provider.
    /// Google checked such an address once, and it may have changed hands
    /// since, so it says nothing of whose it is now and is not given here.
    pub email: Option<String>,
}

/// The ending of Gmail's addresses, which are Google's own.
const GMAIL: &str = "@gmail.com";

/// What Google adds to a token beyond what OpenID Connect names.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Hosted {
    /// The domain of the organisation whose Google Workspace the account is
    /// of. An account that belongs to no organisation has none.
    hd: Option<String>,
}

impl AdditionalClaims for Hosted {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failed {
    /// Google did not answer, or not in a way that can be used.
    Unreachable(String),
    /// Google answered, and what it said does not sign anyone in.
    Refused(String),
    /// Google has not verified that the address is the account's.
    Unverified,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Unreachable(reason) | Failed::Refused(reason) => f.write_str(reason),
            Failed::Unverified => f.write_str("Google has not verified the account's address"),
        }
    }
}

/// `openidconnect`'s client as it is for the usual provider, but for the
/// tokens, which are read with what Google adds to them.
type Client = openidconnect::Client<
    Hosted,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    StandardTokenResponse<
        IdTokenFields<
            Hosted,
            EmptyExtraTokenFields,
            CoreGenderClaim,
            CoreJweContentEncryptionAlgorithm,
            CoreJwsSigningAlgorithm,
        >,
        CoreTokenType,
    >,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// A request that got no answer.
#[derive(Debug)]
struct Unanswered(String);

impl std::fmt::Display for Unanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unanswered {}

/// Google as this server asks it. Every call waits for the network, and
/// belongs on a blocking thread.
pub struct Provider {
    google: Google,
    agent: ureq::Agent,
    known: Mutex<Option<(Instant, CoreProviderMetadata)>>,
}

impl Provider {
    pub fn new(google: Google) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(PATIENCE))
            // What Google answers is read where it was asked for, and
            // nowhere it points to.
            .max_redirects(0)
            // An answer that refuses says why in its body.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            google,
            agent,
            known: Mutex::new(None),
        }
    }

    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Unanswered> {
        let unanswered = |error: &dyn std::fmt::Display| Unanswered(error.to_string());
        let (head, body) = request.into_parts();
        let answered = if head.method == Method::POST {
            let mut request = self.agent.post(&head.uri);
            for (name, value) in &head.headers {
                request = request.header(name, value);
            }
            request.send(&body[..])
        } else {
            let mut request = self.agent.get(&head.uri);
            for (name, value) in &head.headers {
                request = request.header(name, value);
            }
            request.call()
        };
        let mut answered = answered.map_err(|error| unanswered(&error))?;
        let body = answered
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER_BYTES)
            .read_to_vec()
            .map_err(|error| unanswered(&error))?;
        let mut answer = Response::builder().status(answered.status());
        if let Some(kind) = answered.headers().get(CONTENT_TYPE) {
            answer = answer.header(CONTENT_TYPE, kind);
        }
        answer.body(body).map_err(|error| unanswered(&error))
    }

    /// What Google said of its sign-in, if it was asked lately.
    fn known(&self) -> Option<CoreProviderMetadata> {
        self.known
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .as_ref()
            .filter(|(read, _)| read.elapsed() < KEPT_FOR)
            .map(|(_, metadata)| metadata.clone())
    }

    /// Whether [`Provider::start`] would answer without asking Google
    /// anything.
    pub fn knows(&self) -> bool {
        self.known().is_some()
    }

    /// What Google says of its sign-in, asked for if it has not been lately.
    fn client(&self) -> Result<Client, Failed> {
        let metadata = match self.known() {
            Some(metadata) => metadata,
            // Asked without the lock: those who ask while Google is slow
            // wait for it together, not one after the other.
            None => {
                let send = |request| self.send(request);
                let metadata = CoreProviderMetadata::discover(&self.google.issuer, &send).map_err(
                    |error| {
                        Failed::Unreachable(format!(
                            "{} did not say how to sign in with it: {}",
                            self.google.issuer.as_str(),
                            chain(&error)
                        ))
                    },
                )?;
                *self.known.lock().unwrap_or_else(|held| held.into_inner()) =
                    Some((Instant::now(), metadata.clone()));
                metadata
            }
        };
        Ok(Client::from_provider_metadata(
            metadata,
            self.google.client_id.clone(),
            Some(self.google.client_secret.clone()),
        )
        .set_redirect_uri(self.google.redirect.clone()))
    }

    /// Forgets what Google said of its sign-in, so that it is asked again:
    /// a token signed with a key that is not among those kept may be signed
    /// with a newer one.
    fn forget(&self) {
        *self.known.lock().unwrap_or_else(|held| held.into_inner()) = None;
    }

    /// Where to send a browser for its person to sign in, and what to keep
    /// until it comes back.
    pub fn start(&self) -> Result<Started, Failed> {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = self
            .client()?
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".to_owned()))
            // Whoever has several Google accounts says which: otherwise one
            // that has no account here is offered again each time.
            .add_prompt(CoreAuthPrompt::SelectAccount)
            .set_pkce_challenge(challenge)
            .url();
        Ok(Started {
            url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            verifier: verifier.secret().clone(),
        })
    }

    /// Who the code a browser came back with is for. `nonce` and `verifier`
    /// are those of the [`Started`] sign-in it came back from.
    pub fn finish(&self, code: &str, nonce: &str, verifier: &str) -> Result<Identity, Failed> {
        let client = self.client()?;
        let send = |request| self.send(request);
        let answer = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|error| {
                Failed::Unreachable(format!("Google names no token endpoint: {error}"))
            })?
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
            .request(&send)
            .map_err(|error| match error {
                RequestTokenError::ServerResponse(refusal) => {
                    Failed::Refused(format!("Google did not take the code: {refusal}"))
                }
                other => Failed::Unreachable(format!(
                    "Google did not exchange the code: {}",
                    chain(&other)
                )),
            })?;
        let token = answer
            .id_token()
            .ok_or_else(|| Failed::Refused("Google sent no ID token".to_owned()))?;
        let claims = token
            .claims(&client.id_token_verifier(), &Nonce::new(nonce.to_owned()))
            .map_err(|error| {
                self.forget();
                Failed::Refused(format!("the ID token is not one to trust: {error}"))
            })?;
        let Some(email) = claims.email() else {
            return Err(Failed::Refused(
                "the ID token names no email address".to_owned(),
            ));
        };
        if claims.email_verified() != Some(true) {
            return Err(Failed::Unverified);
        }
        let hosted = claims.additional_claims().hd.is_some();
        let answered_for = hosted || email.to_lowercase().ends_with(GMAIL);
        Ok(Identity {
            subject: claims.subject().to_string(),
            email: answered_for.then(|| email.to_string()),
        })
    }
}

/// An error and what caused it, on one line.
fn chain(error: &dyn std::error::Error) -> String {
    let mut said = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        said.push_str(": ");
        said.push_str(&cause.to_string());
        source = cause.source();
    }
    said
}
