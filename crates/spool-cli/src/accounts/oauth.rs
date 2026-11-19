//! What a person lets an assistant do with OAuth: the clients that
//! registered themselves, the codes a person's approval gives them, and the
//! tokens a code is exchanged for.
//!
//! Claude and ChatGPT on the web cannot be handed a token. Each registers
//! itself as a client, sends the person to the server to sign in and
//! approve it, and exchanges the code it is given for tokens of its own,
//! which work at `/mcp` and nowhere else. What it changes is recorded under
//! the person who approved it.
//!
//! An approval, a grant, lasts as long as the sign-in it was given in would,
//! apart from the twelve hours a session has: it ends when the account is
//! turned off, when the password it was given with is replaced, and when
//! the account is moved to another Google account. A code, and each token,
//! is kept as its SHA-256.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    Accounts, BOUND, GOOGLE, MARK, MINUTE_AGO, NOW, PASSWORD, PEOPLE, Refused, code, mark, person,
    sealed,
};
use crate::api::{Grant, Person};

/// What the second version of the accounts file adds.
pub(super) const SCHEMA: &str = "
CREATE TABLE oauth_client (
  id         TEXT PRIMARY KEY,
  name       TEXT,               -- as the client calls itself, which nobody checked
  redirects  TEXT NOT NULL,      -- a JSON list of the addresses a code may be sent to
  created    INTEGER NOT NULL    -- seconds since 1970
);
CREATE TABLE oauth_code (
  secret     TEXT PRIMARY KEY,   -- the SHA-256 of the code
  client     TEXT NOT NULL,
  person     TEXT NOT NULL,
  began_with TEXT NOT NULL,      -- `password:<mark>` or `google:<mark>`, as the session had
  redirect   TEXT NOT NULL,
  challenge  TEXT NOT NULL,      -- PKCE, S256
  expires    INTEGER NOT NULL,
  grant_id   TEXT                -- once it was exchanged: using it again ends that grant
);
CREATE TABLE oauth_grant (
  id           TEXT PRIMARY KEY,
  client       TEXT NOT NULL,
  person       TEXT NOT NULL,
  began_with   TEXT NOT NULL,
  redirect     TEXT NOT NULL,
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  last_used_at TEXT
);
CREATE TABLE oauth_token (
  secret   TEXT PRIMARY KEY,     -- the SHA-256 of the token
  grant_id TEXT NOT NULL,
  refresh  INTEGER NOT NULL,
  expires  INTEGER NOT NULL,
  spent    INTEGER NOT NULL DEFAULT 0  -- a refresh token that was exchanged
);
CREATE INDEX oauth_token_grant ON oauth_token (grant_id);
";

/// What every access token starts with.
pub const ACCESS_PREFIX: &str = "spool_at_";

/// What every refresh token starts with.
pub const REFRESH_PREFIX: &str = "spool_rt_";

/// The one scope there is: to read and change the store as the person who
/// approved. There are no finer permissions.
pub const SCOPE: &str = "spool";

/// How long a code may wait to be exchanged. The client exchanges it as the
/// browser arrives with it.
pub const CODE_SECONDS: i64 = 2 * 60;

/// How long an access token is good for.
pub const ACCESS_SECONDS: i64 = 60 * 60;

/// How long a refresh token is good for. Each one is exchanged once, for an
/// access token and the next refresh token, so a client that is used keeps
/// its approval, and one left alone this long has to be approved again.
pub const REFRESH_SECONDS: i64 = 30 * 24 * 60 * 60;

/// How long a client that nobody approved is kept. Anyone may register one.
const UNAPPROVED_SECONDS: i64 = 24 * 60 * 60;

/// The most clients nobody has approved that are kept at once.
const MAX_UNAPPROVED: i64 = 1_000;

/// A client that registered itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub name: Option<String>,
    pub redirect_uris: Vec<String>,
}

/// A person's approval of a client, as the code it gives is kept.
#[derive(Clone, Debug)]
pub struct Approval<'a> {
    pub client: &'a str,
    pub person: &'a str,
    /// What the person's session began with: [`MARK`] or [`GOOGLE`], and
    /// its mark.
    pub began_with: (&'a str, &'a str),
    pub redirect_uri: &'a str,
    /// The PKCE challenge, the S256 of what the client will present.
    pub challenge: &'a str,
}

/// The tokens a client is given.
#[derive(Clone, Debug)]
pub struct Issued {
    pub access: String,
    pub refresh: String,
    /// The person it acts as.
    pub person: Person,
}

/// Why a code or a refresh token gives nothing, as the client is told.
pub type Turned = &'static str;

impl Accounts {
    /// Keeps a client that registered itself. Clients nobody approved are
    /// forgotten after a day, and only so many are kept at once.
    pub fn register_client(
        &self,
        name: Option<&str>,
        redirect_uris: &[String],
        now: i64,
    ) -> Result<Client, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let unapproved = "id NOT IN (SELECT client FROM oauth_grant)
             AND id NOT IN (SELECT client FROM oauth_code WHERE expires > ?)";
        transaction.execute(
            &format!("DELETE FROM oauth_client WHERE created <= ? AND {unapproved}"),
            params![now - UNAPPROVED_SECONDS, now],
        )?;
        let waiting: i64 = transaction.query_row(
            &format!("SELECT count(*) FROM oauth_client WHERE {unapproved}"),
            [now],
            |row| row.get(0),
        )?;
        if waiting >= MAX_UNAPPROVED {
            return Err(Refused::TooMany(
                "too many clients have registered that nobody approved; try again tomorrow"
                    .to_owned(),
            ));
        }
        let client = Client {
            id: code(),
            name: name.map(str::to_owned),
            redirect_uris: redirect_uris.to_vec(),
        };
        transaction.execute(
            "INSERT INTO oauth_client (id, name, redirects, created) VALUES (?, ?, ?, ?)",
            params![
                client.id,
                client.name,
                serde_json::to_string(&client.redirect_uris).expect("strings are JSON"),
                now
            ],
        )?;
        transaction.commit()?;
        Ok(client)
    }

    pub fn client(&self, id: &str) -> Result<Option<Client>, Refused> {
        let found: Option<(Option<String>, String)> = self
            .connection()
            .query_row(
                "SELECT name, redirects FROM oauth_client WHERE id = ?",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(found.map(|(name, redirects)| Client {
            id: id.to_owned(),
            name,
            redirect_uris: serde_json::from_str(&redirects).unwrap_or_default(),
        }))
    }

    /// Keeps a person's approval, and returns the code that the client
    /// exchanges for tokens.
    pub fn approve(&self, approval: &Approval<'_>, now: i64) -> Result<String, Refused> {
        let code = code();
        let connection = self.connection();
        connection.execute("DELETE FROM oauth_code WHERE expires <= ?", [now])?;
        connection.execute(
            "INSERT INTO oauth_code (secret, client, person, began_with, redirect, challenge, expires)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                sealed(&code),
                approval.client,
                approval.person,
                format!("{}:{}", approval.began_with.0, approval.began_with.1),
                approval.redirect_uri,
                approval.challenge,
                now + CODE_SECONDS
            ],
        )?;
        Ok(code)
    }

    /// Exchanges a code for tokens, if it is one this server gave `client`,
    /// for `redirect_uri` when that is given, and `verifier` is what the
    /// challenge it was given with was made from. A code is exchanged once:
    /// a second try ends what the first was given.
    pub fn exchange(
        &self,
        code: &str,
        client: &str,
        redirect_uri: Option<&str>,
        verifier: &str,
        now: i64,
    ) -> Result<Result<Issued, Turned>, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM oauth_code WHERE expires <= ?", [now])?;
        type Kept = (String, String, String, String, String, Option<String>);
        let kept: Option<Kept> = transaction
            .query_row(
                "SELECT client, person, began_with, redirect, challenge, grant_id
                 FROM oauth_code WHERE secret = ?",
                [sealed(code)],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((given_to, person, began_with, redirect, challenge, exchanged)) = kept else {
            return Ok(Err(
                "that code is not one this server gave, or it is more than two minutes old",
            ));
        };
        if let Some(grant) = exchanged {
            end_grant(&transaction, &grant)?;
            transaction.commit()?;
            return Ok(Err(
                "that code was exchanged already, so the tokens it gave are revoked",
            ));
        }
        if given_to != client {
            return Ok(Err("that code was given to another client"));
        }
        if redirect_uri.is_some_and(|given| given != redirect) {
            return Ok(Err("`redirect_uri` is not the one the code was sent to"));
        }
        if challenged(verifier).as_deref() != Some(challenge.as_str()) {
            return Ok(Err(
                "`code_verifier` is not what the code's `code_challenge` was made from",
            ));
        }
        let Some(person) = self.still(&transaction, &person, &began_with)? else {
            return Ok(Err(
                "the account that approved this can no longer sign in the way it did",
            ));
        };
        let grant = Uuid::now_v7().to_string();
        transaction.execute(
            "INSERT INTO oauth_grant (id, client, person, began_with, redirect, last_used_at)
             VALUES (?, ?, ?, ?, ?, NULL)",
            params![grant, client, person.id, began_with, redirect],
        )?;
        transaction.execute(
            "UPDATE oauth_code SET grant_id = ? WHERE secret = ?",
            params![grant, sealed(code)],
        )?;
        let issued = issue(&transaction, &grant, person, now)?;
        transaction.commit()?;
        Ok(Ok(issued))
    }

    /// Exchanges a refresh token for an access token and the next refresh
    /// token. A refresh token is exchanged once: a second try ends the
    /// approval, since one of the two who tried should not have had it.
    pub fn refresh(
        &self,
        token: &str,
        client: &str,
        now: i64,
    ) -> Result<Result<Issued, Turned>, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let kept: Option<(String, String, String, String, bool)> = transaction
            .query_row(
                "SELECT g.id, g.client, g.person, g.began_with, t.spent
                 FROM oauth_token t JOIN oauth_grant g ON g.id = t.grant_id
                 WHERE t.secret = ? AND t.refresh AND t.expires > ?",
                params![sealed(token), now],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((grant, given_to, person, began_with, spent)) = kept else {
            return Ok(Err(
                "that refresh token is not one this server gave, or it has expired",
            ));
        };
        if given_to != client {
            return Ok(Err("that refresh token was given to another client"));
        }
        let person = match (spent, self.still(&transaction, &person, &began_with)?) {
            (false, Some(person)) => person,
            (spent, _) => {
                end_grant(&transaction, &grant)?;
                transaction.commit()?;
                return Ok(Err(match spent {
                    true => {
                        "that refresh token was used already, so the approval it was part of is revoked"
                    }
                    false => "the account that approved this can no longer sign in the way it did",
                }));
            }
        };
        transaction.execute(
            "UPDATE oauth_token SET spent = 1 WHERE secret = ?",
            [sealed(token)],
        )?;
        let issued = issue(&transaction, &grant, person, now)?;
        transaction.commit()?;
        Ok(Ok(issued))
    }

    /// The person an access token acts as, if it is one this server gave and
    /// their approval stands. Notes that it was used, to the minute.
    pub fn bearer(&self, token: &str, now: i64) -> Result<Option<Person>, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let kept: Option<(String, String, String, bool)> = transaction
            .query_row(
                &format!(
                    "SELECT g.id, g.person, g.began_with,
                            g.last_used_at IS NULL OR g.last_used_at < {MINUTE_AGO}
                     FROM oauth_token t JOIN oauth_grant g ON g.id = t.grant_id
                     WHERE t.secret = ? AND NOT t.refresh AND t.expires > ?"
                ),
                params![sealed(token), now],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((grant, person, began_with, stale)) = kept else {
            return Ok(None);
        };
        let Some(person) = self.still(&transaction, &person, &began_with)? else {
            end_grant(&transaction, &grant)?;
            transaction.commit()?;
            return Ok(None);
        };
        if stale {
            transaction.execute(
                &format!("UPDATE oauth_grant SET last_used_at = {NOW} WHERE id = ?"),
                [&grant],
            )?;
        }
        transaction.commit()?;
        Ok(Some(person))
    }

    /// Takes back a token a client was given, as it asks: a refresh token
    /// with the approval it is part of, an access token by itself. A token
    /// of another client's, or none at all, is left alone.
    pub fn revoke_issued(&self, token: &str, client: &str) -> Result<(), Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let kept: Option<(String, bool)> = transaction
            .query_row(
                "SELECT g.id, t.refresh FROM oauth_token t JOIN oauth_grant g ON g.id = t.grant_id
                 WHERE t.secret = ? AND g.client = ?",
                params![sealed(token), client],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match kept {
            Some((grant, true)) => end_grant(&transaction, &grant)?,
            Some((_, false)) => {
                transaction.execute("DELETE FROM oauth_token WHERE secret = ?", [sealed(token)])?;
            }
            None => {}
        }
        transaction.commit()?;
        Ok(())
    }

    /// Every approval that stands, in the order they were given.
    pub fn grants(&self) -> Result<Vec<Grant>, Refused> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!("{GRANTS} ORDER BY g.created_at, g.id"))?;
        let grants = statement
            .query_map([], grant)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(grants)
    }

    /// Ends an approval, by its ID: its tokens stop working at once.
    pub fn end_grant(&self, id: &str) -> Result<Grant, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let found = transaction
            .query_row(&format!("{GRANTS} WHERE g.id = ?"), [id], grant)
            .optional()?
            .ok_or(Refused::UnknownGrant)?;
        end_grant(&transaction, id)?;
        transaction.commit()?;
        Ok(found)
    }

    /// The person an approval acts as, if it stands: their account is on,
    /// and still signs in the way the session it was given in began.
    fn still(
        &self,
        connection: &Connection,
        id: &str,
        began_with: &str,
    ) -> Result<Option<Person>, Refused> {
        let Some((key, began)) = began_with.split_once(':') else {
            return Ok(None);
        };
        let found: Option<(Person, Option<String>, Option<String>)> = connection
            .query_row(&format!("{PEOPLE} WHERE id = ?"), [id], |row| {
                Ok((person(row)?, row.get(PASSWORD)?, row.get(BOUND)?))
            })
            .optional()?;
        let Some((person, hash, bound)) = found else {
            return Ok(None);
        };
        let same = match key {
            MARK => hash.is_some_and(|hash| mark(&hash) == began),
            // A server that no longer signs people in with Google has nobody
            // signed in with it.
            GOOGLE => self.google && bound.as_deref() == Some(began),
            _ => false,
        };
        Ok((same && person.active).then_some(person))
    }
}

const GRANTS: &str = "SELECT g.id, g.person, c.name, g.redirect, g.created_at, g.last_used_at
     FROM oauth_grant g JOIN oauth_client c ON c.id = g.client";

fn grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<Grant> {
    Ok(Grant {
        id: row.get(0)?,
        person: row.get(1)?,
        client_name: row.get(2)?,
        redirect_uri: row.get(3)?,
        created_at: row.get(4)?,
        last_used_at: row.get(5)?,
    })
}

/// Gives an approval an access token and a refresh token, and forgets the
/// tokens that have expired and the approvals left with none.
fn issue(
    transaction: &Transaction<'_>,
    grant: &str,
    person: Person,
    now: i64,
) -> Result<Issued, Refused> {
    transaction.execute("DELETE FROM oauth_token WHERE expires <= ?", [now])?;
    transaction.execute(
        "DELETE FROM oauth_grant WHERE id <> ? AND id NOT IN (SELECT grant_id FROM oauth_token)",
        [grant],
    )?;
    let access = format!("{ACCESS_PREFIX}{}", code());
    let refresh = format!("{REFRESH_PREFIX}{}", code());
    for (token, is_refresh, lasts) in [
        (&access, false, ACCESS_SECONDS),
        (&refresh, true, REFRESH_SECONDS),
    ] {
        transaction.execute(
            "INSERT INTO oauth_token (secret, grant_id, refresh, expires) VALUES (?, ?, ?, ?)",
            params![sealed(token), grant, is_refresh, now + lasts],
        )?;
    }
    Ok(Issued {
        access,
        refresh,
        person,
    })
}

/// Ends the approvals a person gave, and the codes waiting to be exchanged
/// for one: all of them, or with `began_with`, those given in a sign-in that
/// began that way ([`MARK`] or [`GOOGLE`]).
pub(super) fn end_approvals(
    connection: &Connection,
    person: &str,
    began_with: Option<&str>,
) -> Result<(), Refused> {
    let like = format!("{}:%", began_with.unwrap_or("%"));
    connection.execute(
        "DELETE FROM oauth_token WHERE grant_id IN
           (SELECT id FROM oauth_grant WHERE person = ? AND began_with LIKE ?)",
        params![person, like],
    )?;
    for table in ["oauth_grant", "oauth_code"] {
        connection.execute(
            &format!("DELETE FROM {table} WHERE person = ? AND began_with LIKE ?"),
            params![person, like],
        )?;
    }
    Ok(())
}

fn end_grant(connection: &Connection, grant: &str) -> Result<(), Refused> {
    connection.execute("DELETE FROM oauth_token WHERE grant_id = ?", [grant])?;
    connection.execute("DELETE FROM oauth_grant WHERE id = ?", [grant])?;
    Ok(())
}

/// The S256 challenge of a PKCE verifier: the SHA-256 of it in base64url,
/// unpadded. `None` if it is not a verifier: 43 to 128 of the characters
/// RFC 7636 allows.
fn challenged(verifier: &str) -> Option<String> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "-._~".contains(c);
    if !(43..=128).contains(&verifier.len()) || !verifier.chars().all(allowed) {
        return None;
    }
    Some(base64url(&Sha256::digest(verifier)))
}

pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .chain([0, 0].iter())
            .take(3)
            .fold(0u32, |group, byte| (group << 8) | u32::from(*byte));
        for index in 0..=chunk.len() {
            text.push(ALPHABET[((group >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::{Change, New};
    use super::*;

    const PASSWORD: &str = "a long enough one";
    const VERIFIER: &str = "a-verifier-of-forty-three-characters-or-more";
    const BACK: &str = "https://claude.ai/api/mcp/auth_callback";
    const NOW: i64 = 1_800_000_000;

    fn challenge() -> String {
        challenged(VERIFIER).unwrap()
    }

    /// An account file with Ada, who signed in with a password, and a client.
    fn ready() -> (Accounts, Person, String, Client) {
        let accounts = Accounts::in_memory();
        let entered = accounts.first("ada", "Ada", PASSWORD).unwrap();
        let client = accounts
            .register_client(Some("Claude"), &[BACK.to_owned()], NOW)
            .unwrap();
        (accounts, entered.person, entered.mark, client)
    }

    fn approved(accounts: &Accounts, ada: &Person, mark: &str, client: &Client) -> String {
        let challenge = challenge();
        let approval = Approval {
            client: &client.id,
            person: &ada.id,
            began_with: (MARK, mark),
            redirect_uri: BACK,
            challenge: &challenge,
        };
        accounts.approve(&approval, NOW).unwrap()
    }

    fn exchanged(
        accounts: &Accounts,
        code: &str,
        client: &Client,
        at: i64,
    ) -> Result<Issued, Turned> {
        accounts
            .exchange(code, &client.id, Some(BACK), VERIFIER, at)
            .unwrap()
    }

    #[test]
    fn the_challenge_is_rfc_7636s() {
        // The example of RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenged(verifier).as_deref(),
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        assert_eq!(challenged(&verifier[1..]), None);
        assert!(challenged(&"a".repeat(128)).is_some());
        assert_eq!(challenged(&"a".repeat(129)), None);
        assert_eq!(challenged(&format!("{}+", "a".repeat(43))), None);
    }

    #[test]
    fn a_code_is_exchanged_once_by_its_client_with_its_verifier() {
        let (accounts, ada, mark, client) = ready();
        let other = accounts
            .register_client(None, &[BACK.to_owned()], NOW)
            .unwrap();
        let code = approved(&accounts, &ada, &mark, &client);

        let refused = |client: &Client, redirect: Option<&str>, verifier: &str| {
            accounts
                .exchange(&code, &client.id, redirect, verifier, NOW)
                .unwrap()
                .unwrap_err()
        };
        assert!(refused(&other, Some(BACK), VERIFIER).contains("another client"));
        assert!(refused(&client, Some("https://chatgpt.com/x"), VERIFIER).contains("redirect_uri"));
        assert!(refused(&client, Some(BACK), &"b".repeat(43)).contains("code_verifier"));
        assert!(exchanged(&accounts, "not-a-code", &client, NOW).is_err());

        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();
        assert_eq!(issued.person, ada);
        assert!(issued.access.starts_with(ACCESS_PREFIX));
        assert!(issued.refresh.starts_with(REFRESH_PREFIX));
        assert_eq!(
            accounts.bearer(&issued.access, NOW).unwrap(),
            Some(ada.clone())
        );
        // A refresh token is not an access token, and the other way round.
        assert_eq!(accounts.bearer(&issued.refresh, NOW).unwrap(), None);
        assert!(
            accounts
                .refresh(&issued.access, &client.id, NOW)
                .unwrap()
                .is_err()
        );

        // A second exchange of the code ends what the first was given.
        let again = exchanged(&accounts, &code, &client, NOW + 1).unwrap_err();
        assert!(again.contains("exchanged already"), "{again}");
        assert_eq!(accounts.bearer(&issued.access, NOW + 1).unwrap(), None);
        assert!(accounts.grants().unwrap().is_empty());
    }

    #[test]
    fn a_code_lasts_two_minutes() {
        let (accounts, ada, mark, client) = ready();
        let code = approved(&accounts, &ada, &mark, &client);
        assert!(exchanged(&accounts, &code, &client, NOW + CODE_SECONDS).is_err());
    }

    #[test]
    fn tokens_expire_and_a_refresh_token_is_exchanged_once() {
        let (accounts, ada, mark, client) = ready();
        let code = approved(&accounts, &ada, &mark, &client);
        let first = exchanged(&accounts, &code, &client, NOW).unwrap();
        let later = NOW + ACCESS_SECONDS;
        assert_eq!(
            accounts.bearer(&first.access, later - 1).unwrap(),
            Some(ada.clone())
        );
        assert_eq!(accounts.bearer(&first.access, later).unwrap(), None);

        let other = accounts
            .register_client(None, &[BACK.to_owned()], NOW)
            .unwrap();
        let refused = accounts.refresh(&first.refresh, &other.id, later).unwrap();
        assert!(refused.unwrap_err().contains("another client"));

        let second = accounts
            .refresh(&first.refresh, &client.id, later)
            .unwrap()
            .unwrap();
        assert_eq!(
            accounts.bearer(&second.access, later).unwrap(),
            Some(ada.clone())
        );
        assert_eq!(accounts.grants().unwrap().len(), 1);

        // The spent token again: whoever has it should not, and the approval ends.
        let reused = accounts
            .refresh(&first.refresh, &client.id, later + 1)
            .unwrap();
        assert!(reused.unwrap_err().contains("used already"));
        assert_eq!(accounts.bearer(&second.access, later + 1).unwrap(), None);
        assert!(
            accounts
                .refresh(&second.refresh, &client.id, later + 1)
                .unwrap()
                .is_err()
        );
        assert!(accounts.grants().unwrap().is_empty());

        // A refresh token left alone for thirty days has expired.
        let code = approved(&accounts, &ada, &mark, &client);
        let third = exchanged(&accounts, &code, &client, NOW).unwrap();
        let expired = accounts
            .refresh(&third.refresh, &client.id, NOW + REFRESH_SECONDS)
            .unwrap();
        assert!(expired.unwrap_err().contains("expired"));
    }

    #[test]
    fn an_approval_ends_with_the_sign_in_it_was_given_in() {
        let (accounts, ada, mark, client) = ready();
        let code = approved(&accounts, &ada, &mark, &client);
        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();
        let reset = Change {
            password: Some("the next long one".to_owned()),
            ..Change::default()
        };
        accounts.change(&ada.id, reset).unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);
        assert!(
            accounts
                .refresh(&issued.refresh, &client.id, NOW)
                .unwrap()
                .is_err()
        );

        // An approval given before the password changed is no good either.
        let code = approved(&accounts, &ada, &mark, &client);
        let turned = exchanged(&accounts, &code, &client, NOW).unwrap_err();
        assert!(turned.contains("no longer sign in"), "{turned}");

        // Turning the account off ends it too.
        let entered = accounts
            .verify("ada", "the next long one")
            .unwrap()
            .unwrap();
        let code = approved(&accounts, &ada, &entered.mark, &client);
        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();
        accounts
            .add(New {
                admin: true,
                ..New {
                    username: "bob".to_owned(),
                    name: "Bob".to_owned(),
                    password: Some(PASSWORD.to_owned()),
                    ..New::default()
                }
            })
            .unwrap();
        let waiting = approved(&accounts, &ada, &entered.mark, &client);
        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        accounts.change(&ada.id, off).unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);

        // Turned on again, the account signs in afresh: what it approved
        // before stays ended, and so does an approval not yet exchanged.
        let on = Change {
            active: Some(true),
            ..Change::default()
        };
        accounts.change(&ada.id, on).unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);
        assert!(
            accounts
                .refresh(&issued.refresh, &client.id, NOW)
                .unwrap()
                .is_err()
        );
        assert!(exchanged(&accounts, &waiting, &client, NOW).is_err());
        assert!(accounts.grants().unwrap().is_empty());
    }

    #[test]
    fn an_approval_given_with_google_ends_when_the_server_no_longer_has_google() {
        let accounts = Accounts::in_memory().with_google(true);
        accounts.first("ada", "Ada", PASSWORD).unwrap();
        let grace = accounts
            .add(New {
                username: "grace".to_owned(),
                name: "Grace".to_owned(),
                email: Some("grace@example.com".to_owned()),
                ..New::default()
            })
            .unwrap();
        let (_, bound) = accounts
            .enter_with_google("g-1", Some("grace@example.com"))
            .unwrap()
            .unwrap();
        let client = accounts
            .register_client(None, &[BACK.to_owned()], NOW)
            .unwrap();
        let challenge = challenge();
        let approval = Approval {
            client: &client.id,
            person: &grace.id,
            began_with: (GOOGLE, &bound),
            redirect_uri: BACK,
            challenge: &challenge,
        };
        let code = accounts.approve(&approval, NOW).unwrap();
        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();
        assert_eq!(
            accounts.bearer(&issued.access, NOW).unwrap(),
            Some(grace.clone())
        );
        let without = accounts.clone().with_google(false);
        assert_eq!(without.bearer(&issued.access, NOW).unwrap(), None);

        // Another email ends it for good, even when the same Google account
        // is bound again under the email it had.
        let other = |email: &str| Change {
            email: Some(email.to_owned()),
            ..Change::default()
        };
        accounts
            .change(&grace.id, other("hopper@example.com"))
            .unwrap();
        accounts
            .change(&grace.id, other("grace@example.com"))
            .unwrap();
        accounts
            .enter_with_google("g-1", Some("grace@example.com"))
            .unwrap()
            .unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);
        assert!(accounts.grants().unwrap().is_empty());
    }

    #[test]
    fn a_client_revokes_its_own_tokens_and_an_admin_any_approval() {
        let (accounts, ada, mark, client) = ready();
        let other = accounts
            .register_client(None, &[BACK.to_owned()], NOW)
            .unwrap();
        let code = approved(&accounts, &ada, &mark, &client);
        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();

        accounts.revoke_issued(&issued.refresh, &other.id).unwrap();
        assert!(accounts.bearer(&issued.access, NOW).unwrap().is_some());
        accounts.revoke_issued(&issued.access, &client.id).unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);
        let next = accounts
            .refresh(&issued.refresh, &client.id, NOW)
            .unwrap()
            .unwrap();
        accounts.revoke_issued(&next.refresh, &client.id).unwrap();
        assert_eq!(accounts.bearer(&next.access, NOW).unwrap(), None);

        let code = approved(&accounts, &ada, &mark, &client);
        let issued = exchanged(&accounts, &code, &client, NOW).unwrap();
        let listed = accounts.grants().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].person, ada.id);
        assert_eq!(listed[0].client_name.as_deref(), Some("Claude"));
        assert_eq!(listed[0].redirect_uri, BACK);
        assert!(listed[0].last_used_at.is_none());
        accounts.bearer(&issued.access, NOW).unwrap();
        assert!(accounts.grants().unwrap()[0].last_used_at.is_some());
        accounts.end_grant(&listed[0].id).unwrap();
        assert_eq!(accounts.bearer(&issued.access, NOW).unwrap(), None);
        assert_eq!(
            accounts.end_grant(&listed[0].id),
            Err(Refused::UnknownGrant)
        );
    }

    #[test]
    fn clients_nobody_approved_are_kept_a_day_and_so_many_at_once() {
        let accounts = Accounts::in_memory();
        let back = [BACK.to_owned()];
        for _ in 0..MAX_UNAPPROVED {
            accounts.register_client(None, &back, NOW).unwrap();
        }
        let refused = accounts.register_client(None, &back, NOW + 1);
        assert!(matches!(refused, Err(Refused::TooMany(_))), "{refused:?}");
        let kept = accounts
            .register_client(None, &back, NOW + UNAPPROVED_SECONDS)
            .unwrap();
        assert_eq!(accounts.client(&kept.id).unwrap(), Some(kept));
    }
}
