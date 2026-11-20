//! Who is signing in: the account a password or a Google account is the
//! way into, and whether a session that began with one still stands.

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::accounts::{Accounts, Refused, code};
use super::people::{BOUND, PASSWORD, PEOPLE, person};
use super::sessions::end_sessions;
use crate::api::Person;

/// The fewest characters a password may have.
pub const MIN_PASSWORD: usize = 12;

/// Someone whose password was just checked or set, and the mark of it that
/// their session keeps. A session is good while the account's password is
/// still the one with that mark, so one that begins while the password is
/// being replaced is of the old password, and is not good.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entered {
    pub person: Person,
    pub mark: String,
}

impl Accounts {
    /// The person a session belongs to, if their account is on and their
    /// password is still the one the session began with.
    pub fn holder(&self, id: &str, began_with: &str) -> Result<Option<Person>, Refused> {
        let found: Option<(Person, Option<String>)> = self
            .connection()
            .query_row(&format!("{PEOPLE} WHERE id = ?"), [id], |row| {
                Ok((person(row)?, row.get(PASSWORD)?))
            })
            .optional()?;
        Ok(found.and_then(|(person, hash)| {
            let same = hash.is_some_and(|hash| mark(&hash) == began_with);
            (same && person.active).then_some(person)
        }))
    }

    /// The person a session that began with Google belongs to, if their
    /// account is on and is still bound to a Google account as it was when
    /// the session began: `began_with` is what [`Accounts::enter_with_google`]
    /// answered then.
    pub fn holder_with_google(
        &self,
        id: &str,
        began_with: &str,
    ) -> Result<Option<Person>, Refused> {
        let found = self
            .connection()
            .query_row(
                &format!("{PEOPLE} WHERE id = ? AND bound = ?"),
                [id, began_with],
                person,
            )
            .optional()?;
        Ok(found.filter(|person| person.active))
    }

    /// The person a Google account is, if they have an account here that is
    /// on. `subject` is what Google calls the account, which stays the same
    /// whatever becomes of its address. `email` is the account's address if
    /// it is one Google answers for, so that whoever has the Google account
    /// has the address: without one, only an account that is already that
    /// Google account's is found.
    ///
    /// The first time, the account is the one an admin gave that email, and
    /// from then on it is that Google account's: another Google account that
    /// later has the address is not let in.
    ///
    /// Returns the person and what a session of theirs begins with: the mark
    /// of the account being that Google account's. It is made when the two
    /// are bound, so a session from before an admin changed the email stays
    /// ended, even when the same Google account is bound again.
    pub fn enter_with_google(
        &self,
        subject: &str,
        email: Option<&str>,
    ) -> Result<Option<(Person, String)>, Refused> {
        let email = email.map(|email| email.trim().to_lowercase());
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        // The account and the mark of its Google account, if it has one.
        let bound = |row: &rusqlite::Row<'_>| Ok((person(row)?, row.get(BOUND)?));
        let known: Option<(Person, Option<String>)> = transaction
            .query_row(&format!("{PEOPLE} WHERE google = ?"), [subject], bound)
            .optional()?;
        let found = match (known, email) {
            (Some(known), _) => Some(known),
            (None, Some(email)) => transaction
                .query_row(
                    &format!("{PEOPLE} WHERE email = ? AND google IS NULL"),
                    [&email],
                    bound,
                )
                .optional()?,
            (None, None) => None,
        };
        let Some((person, mark)) = found.filter(|(person, _)| person.active) else {
            return Ok(None);
        };
        let mark = match mark {
            Some(mark) => mark,
            None => {
                let mark = code();
                transaction.execute(
                    "UPDATE person SET google = ?, bound = ? WHERE id = ?",
                    [subject, &mark, person.id.as_str()],
                )?;
                mark
            }
        };
        transaction.commit()?;
        Ok(Some((person, mark)))
    }

    /// The person with this username, if the password is theirs and their
    /// account is on. Takes as long whether or not there is such a person.
    pub fn verify(&self, username: &str, password: &str) -> Result<Option<Entered>, Refused> {
        let username = username.trim().to_lowercase();
        let found: Option<(Person, Option<String>)> = self
            .connection()
            .query_row(
                &format!("{PEOPLE} WHERE username = ?"),
                [&username],
                |row| Ok((person(row)?, row.get(PASSWORD)?)),
            )
            .optional()?;
        match found {
            Some((person, Some(hash))) => {
                let right = password_auth::verify_password(password, &hash).is_ok();
                // The mark is of the hash that was checked, which by now
                // may not be the account's.
                Ok((right && person.active).then(|| Entered {
                    person,
                    mark: mark(&hash),
                }))
            }
            _ => {
                // The work a real check does, so that the time taken does
                // not say whether the username exists.
                let _ = password_auth::verify_password(password, nobody());
                Ok(None)
            }
        }
    }

    /// Changes a person's own password, given the one they have, and
    /// returns the mark of the new one. Their sessions end.
    pub fn change_password(&self, id: &str, current: &str, new: &str) -> Result<String, Refused> {
        let stored: Option<Option<String>> = self
            .connection()
            .query_row("SELECT password FROM person WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let stored = stored
            .ok_or(Refused::Unknown)?
            .ok_or(Refused::WrongPassword)?;
        password_auth::verify_password(current, &stored).map_err(|_| Refused::WrongPassword)?;
        let hash = hashed(new)?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        // Only over the password that was checked: one an admin set in the
        // meantime stays.
        let changed = transaction.execute(
            "UPDATE person SET password = ? WHERE id = ? AND password = ?",
            params![hash, id, stored],
        )?;
        if changed == 0 {
            return Err(Refused::WrongPassword);
        }
        end_sessions(&transaction, id)?;
        transaction.commit()?;
        Ok(mark(&hash))
    }
}

pub(super) fn hashed(password: &str) -> Result<String, Refused> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(Refused::Invalid(format!(
            "a password has at least {MIN_PASSWORD} characters"
        )));
    }
    Ok(password_auth::generate_hash(password))
}

/// What a session keeps of the password it began with: the SHA-256 of its
/// stored hash, which changes whenever the password is set.
pub(super) fn mark(hash: &str) -> String {
    hex::encode(Sha256::digest(hash))
}

/// A hash no password matches, to check against when there is no account.
fn nobody() -> &'static str {
    static HASH: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| password_auth::generate_hash(Uuid::now_v7().as_bytes()));
    &HASH
}

#[cfg(test)]
mod tests {
    use super::super::testing::{PASSWORD, email, new, with_email};
    use super::super::{Change, New};
    use super::*;

    #[test]
    fn a_google_account_is_whoever_an_admin_gave_its_email() {
        let accounts = Accounts::in_memory().with_google(true);
        accounts.first("ada", "Ada", PASSWORD).unwrap();
        let grace = accounts
            .add(with_email("grace", " Grace@Example.com "))
            .unwrap();
        assert_eq!(grace.email.as_deref(), Some("grace@example.com"));
        // Nobody has a password to try.
        assert_eq!(accounts.verify("grace", "").unwrap(), None);

        // Who a Google account is, and what a session of theirs begins with.
        let begin =
            |subject: &str, email: &str| accounts.enter_with_google(subject, Some(email)).unwrap();
        let enter = |subject: &str, email: &str| begin(subject, email).map(|(person, _)| person);
        let holder = |began_with: &str| accounts.holder_with_google(&grace.id, began_with).unwrap();
        assert_eq!(enter("g-1", "nobody@example.com"), None);
        let (entered, first) = begin("g-1", "GRACE@example.com").unwrap();
        assert_eq!(entered, grace);
        assert_eq!(holder(&first), Some(grace.clone()));
        // What Google calls the account is known to others, and begins
        // nothing.
        assert_eq!(holder("g-1"), None);
        assert_eq!(holder(""), None);
        // Each sign-in of the same Google account is a session of the same
        // kind.
        assert_eq!(begin("g-1", "grace@example.com").unwrap().1, first);
        // An address Google does not answer for finds the account that is
        // already that Google account's, and no other.
        let without = |subject: &str| accounts.enter_with_google(subject, None).unwrap();
        assert_eq!(without("g-1"), Some((grace.clone(), first.clone())));
        assert_eq!(without("g-2"), None);

        // The account is that Google account's now: whatever its address
        // becomes, and whoever else comes to have the address.
        assert_eq!(enter("g-1", "hopper@example.com"), Some(grace.clone()));
        assert_eq!(enter("g-2", "grace@example.com"), None);

        // Another email is another Google account's.
        let moved = accounts
            .change(&grace.id, email("hopper@example.com"))
            .unwrap();
        assert_eq!(moved.email.as_deref(), Some("hopper@example.com"));
        assert_eq!(holder(&first), None);
        assert_eq!(enter("g-2", "grace@example.com"), None);
        // The Google account it was before may be the one with the new
        // address. What it began before the change stays ended.
        let (entered, second) = begin("g-1", "hopper@example.com").unwrap();
        assert_eq!(entered, moved);
        assert_ne!(second, first);
        assert_eq!(holder(&first), None);
        assert_eq!(holder(&second), Some(moved.clone()));

        let moved = accounts
            .change(&grace.id, email("g.hopper@example.com"))
            .unwrap();
        let (entered, third) = begin("g-2", "g.hopper@example.com").unwrap();
        assert_eq!(entered, moved);
        assert_eq!(holder(&second), None);

        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        accounts.change(&grace.id, off).unwrap();
        assert_eq!(enter("g-2", "g.hopper@example.com"), None);
        assert_eq!(holder(&third), None);
    }

    #[test]
    fn a_password_signs_in_only_its_own_active_account() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        let bob = accounts.add(new("bob", "Bob", "another long one")).unwrap();
        let entered = accounts.verify(" ADA ", PASSWORD).unwrap();
        assert_eq!(entered.map(|entered| entered.person), Some(ada.clone()));
        assert_eq!(accounts.verify("ada", "another long one").unwrap(), None);
        assert_eq!(accounts.verify("nobody", PASSWORD).unwrap(), None);

        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        assert!(!accounts.change(&bob.id, off.clone()).unwrap().active);
        assert_eq!(accounts.verify("bob", "another long one").unwrap(), None);
        // Someone must be left to manage people.
        assert_eq!(accounts.change(&ada.id, off), Err(Refused::LastAdmin));
    }

    #[test]
    fn a_session_lasts_only_as_long_as_the_password_it_began_with() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        // A sign-in whose password was checked just before an admin set a new one.
        let entered = accounts.verify("ada", PASSWORD).unwrap().unwrap();
        assert_eq!(
            accounts.holder(&ada.id, &entered.mark).unwrap(),
            Some(ada.clone())
        );
        let reset = Change {
            password: Some("the next long one".to_owned()),
            ..Change::default()
        };
        accounts.change(&ada.id, reset).unwrap();
        // The session it would start is of the password that was replaced.
        assert_eq!(accounts.holder(&ada.id, &entered.mark).unwrap(), None);

        let entered = accounts
            .verify("ada", "the next long one")
            .unwrap()
            .unwrap();
        let mark = accounts
            .change_password(&ada.id, "the next long one", "one more long one")
            .unwrap();
        assert_eq!(accounts.holder(&ada.id, &entered.mark).unwrap(), None);
        assert_eq!(accounts.holder(&ada.id, &mark).unwrap(), Some(ada.clone()));

        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        accounts
            .add(New {
                admin: true,
                ..new("bob", "Bob", PASSWORD)
            })
            .unwrap();
        accounts.change(&ada.id, off).unwrap();
        assert_eq!(accounts.holder(&ada.id, &mark).unwrap(), None);
    }

    #[test]
    fn a_new_password_needs_the_old_one() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        assert_eq!(
            accounts.change_password(&ada.id, "not it at all", "the next long one"),
            Err(Refused::WrongPassword)
        );
        accounts
            .change_password(&ada.id, PASSWORD, "the next long one")
            .unwrap();
        assert_eq!(accounts.verify("ada", PASSWORD).unwrap(), None);
        assert!(
            accounts
                .verify("ada", "the next long one")
                .unwrap()
                .is_some()
        );
    }
}
