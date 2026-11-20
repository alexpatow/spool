//! The tokens of callers that are not people: making one, revoking it, and
//! telling which one a caller presents.

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::accounts::{Accounts, MINUTE_AGO, NOW, Refused, code};
use super::people::MAX_USERNAME;
use crate::api::Token;

/// What every token starts with, so that one found where it should not be
/// can be told for what it is.
pub const TOKEN_PREFIX: &str = "spool_";

impl Accounts {
    /// Makes a token for a caller that is not a person, and returns it with
    /// the token itself, which is kept nowhere: only its hash is.
    pub fn add_token(&self, name: &str, by: Option<&str>) -> Result<(Token, String), Refused> {
        let name = name.trim().to_lowercase();
        let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c);
        if name.is_empty() || name.len() > MAX_USERNAME || !name.chars().all(plain) {
            return Err(Refused::Invalid(
                "a token's name is 1 to 64 letters, digits, `.`, `_` or `-`".to_owned(),
            ));
        }
        let secret = format!("{TOKEN_PREFIX}{}", code());
        let id = Uuid::now_v7().to_string();
        let connection = self.connection();
        let inserted = connection.execute(
            "INSERT INTO token (id, name, secret, created_by) VALUES (?, ?, ?, ?)
             ON CONFLICT (name) DO NOTHING",
            params![id, name, sealed(&secret), by],
        )?;
        if inserted == 0 {
            return Err(Refused::TokenTaken(name));
        }
        let token = connection.query_row(&format!("{TOKENS} WHERE id = ?"), [&id], token)?;
        Ok((token, secret))
    }

    /// Every token, in the order they were made.
    pub fn tokens(&self) -> Result<Vec<Token>, Refused> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!("{TOKENS} ORDER BY created_at, name"))?;
        let tokens = statement
            .query_map([], token)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tokens)
    }

    /// Takes a token away, by its ID. What it changed stays in the log under
    /// its name.
    pub fn revoke_token(&self, id: &str) -> Result<Token, Refused> {
        self.revoke("id", id)
    }

    /// As [`Accounts::revoke_token`], by its name.
    pub fn revoke_token_named(&self, name: &str) -> Result<Token, Refused> {
        self.revoke("name", &name.trim().to_lowercase())
    }

    fn revoke(&self, column: &str, value: &str) -> Result<Token, Refused> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let found = transaction
            .query_row(&format!("{TOKENS} WHERE {column} = ?"), [value], token)
            .optional()?
            .ok_or(Refused::UnknownToken)?;
        transaction.execute("DELETE FROM token WHERE id = ?", [&found.id])?;
        transaction.commit()?;
        Ok(found)
    }

    /// The name of the token `presented` is, if it is one. Notes that it was
    /// used, to the minute: a token is presented with every request, and
    /// that is not worth a write each time.
    pub fn agent(&self, presented: &str) -> Result<Option<String>, Refused> {
        let secret = sealed(presented);
        let connection = self.connection();
        let found: Option<(String, bool)> = connection
            .query_row(
                &format!(
                    "SELECT name, last_used_at IS NULL OR last_used_at < {MINUTE_AGO}
                     FROM token WHERE secret = ?"
                ),
                [&secret],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((name, stale)) = found else {
            return Ok(None);
        };
        if stale {
            connection.execute(
                &format!("UPDATE token SET last_used_at = {NOW} WHERE secret = ?"),
                [&secret],
            )?;
        }
        Ok(Some(name))
    }
}

const TOKENS: &str = "SELECT id, name, created_at, created_by, last_used_at FROM token";

fn token(row: &rusqlite::Row<'_>) -> rusqlite::Result<Token> {
    Ok(Token {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        created_by: row.get(3)?,
        last_used_at: row.get(4)?,
    })
}

/// What is kept of a token: its SHA-256. A token is 32 random bytes, so
/// nothing is gained by a hash that is slow to make, and it is looked up by
/// this.
pub(super) fn sealed(token: &str) -> String {
    hex::encode(Sha256::digest(token))
}

#[cfg(test)]
mod tests {
    use super::super::testing::PASSWORD;
    use super::*;

    #[test]
    fn a_token_is_known_by_what_it_is_and_named_in_what_it_does() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        let (token, secret) = accounts.add_token(" Load ", Some(&ada.id)).unwrap();
        assert_eq!(token.name, "load");
        assert_eq!(token.created_by.as_deref(), Some(ada.id.as_str()));
        assert_eq!(token.last_used_at, None);
        assert!(secret.starts_with(TOKEN_PREFIX) && secret.len() == TOKEN_PREFIX.len() + 64);

        assert_eq!(accounts.agent(&secret).unwrap().as_deref(), Some("load"));
        for wrong in [
            "",
            "load",
            TOKEN_PREFIX,
            &secret[..secret.len() - 1],
            &format!("{secret}0"),
        ] {
            assert_eq!(accounts.agent(wrong).unwrap(), None, "{wrong}");
        }
        // The file does not hold it.
        let kept: String = accounts
            .connection()
            .query_row("SELECT secret FROM token", [], |row| row.get(0))
            .unwrap();
        assert!(kept != secret && !kept.contains(&secret[TOKEN_PREFIX.len()..]));

        let listed = accounts.tokens().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, token.id);
        assert!(listed[0].last_used_at.is_some());
    }

    #[test]
    fn a_token_has_a_name_of_its_own() {
        let accounts = Accounts::in_memory();
        accounts.add_token("load", None).unwrap();
        assert_eq!(
            accounts.add_token("LOAD", None).unwrap_err(),
            Refused::TokenTaken("load".to_owned())
        );
        for name in ["", " ", "two words", "agent:api", &"a".repeat(65)] {
            let refused = accounts.add_token(name, None).unwrap_err();
            assert!(
                matches!(refused, Refused::Invalid(_)),
                "{name}: {refused:?}"
            );
        }
        let (other, _) = accounts.add_token("review-bot", None).unwrap();
        let names = |tokens: Vec<Token>| {
            tokens
                .into_iter()
                .map(|token| token.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(accounts.tokens().unwrap()), ["load", "review-bot"]);
        assert_eq!(other.created_by, None);
    }

    #[test]
    fn a_revoked_token_is_no_longer_one() {
        let accounts = Accounts::in_memory();
        let (first, secret) = accounts.add_token("load", None).unwrap();
        let (_, other) = accounts.add_token("review-bot", None).unwrap();
        assert_eq!(accounts.revoke_token(&first.id).unwrap().name, "load");
        assert_eq!(accounts.agent(&secret).unwrap(), None);
        assert_eq!(
            accounts.agent(&other).unwrap().as_deref(),
            Some("review-bot")
        );
        assert_eq!(accounts.revoke_token(&first.id), Err(Refused::UnknownToken));

        // The name is free again, and the token that had it stays revoked.
        let (_, next) = accounts.add_token("load", None).unwrap();
        assert_eq!(accounts.agent(&secret).unwrap(), None);
        assert_eq!(accounts.agent(&next).unwrap().as_deref(), Some("load"));
        assert_eq!(
            accounts.revoke_token_named(" Review-Bot ").unwrap().name,
            "review-bot"
        );
        assert_eq!(
            accounts.revoke_token_named("review-bot"),
            Err(Refused::UnknownToken)
        );
    }
}
