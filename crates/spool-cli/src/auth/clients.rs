//! The OAuth clients that registered themselves: an assistant does, before
//! it asks a person to approve it.

use rusqlite::{OptionalExtension, params};

use super::accounts::{Accounts, Refused, code};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACK: &str = "https://claude.ai/api/mcp/auth_callback";
    const NOW: i64 = 1_800_000_000;

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
