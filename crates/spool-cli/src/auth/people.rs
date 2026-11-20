//! The people who have an account: adding one, changing one, and what a
//! username, a name and an email may be.

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::accounts::{Accounts, Refused};
use super::grants;
use super::sessions::{GOOGLE, PERSON, end_sessions};
use super::sign_in::{Entered, hashed, mark};
use crate::api::Person;

/// The most bytes a username has.
pub const MAX_USERNAME: usize = 64;

/// The most bytes an email has.
pub const MAX_EMAIL: usize = 254;

/// A person to add. They sign in with the password, with Google by the
/// email, or with either: one of the two is needed.
#[derive(Clone, Debug, Default)]
pub struct New {
    pub username: String,
    pub name: String,
    pub password: Option<String>,
    pub email: Option<String>,
    pub admin: bool,
}

/// What to change about a person. A field left out stays as it is.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub name: Option<String>,
    pub password: Option<String>,
    /// The email they sign in with Google by. An empty one takes it away.
    pub email: Option<String>,
    pub admin: Option<bool>,
    pub active: Option<bool>,
}

impl Accounts {
    /// Whether no admin can sign in yet, so that nobody could add anyone.
    /// The account `spool dev` works as has no password and is not one who
    /// can: a store that was developed on still gets its first account.
    pub fn needs_setup(&self) -> Result<bool, Refused> {
        Ok(self.admins(&self.connection(), true)? == 0)
    }

    /// How many admins are on: with `signs_in`, only those who can sign in,
    /// with a password or, where the server signs people in with Google, by
    /// their email.
    fn admins(&self, connection: &Connection, signs_in: bool) -> Result<i64, Refused> {
        Ok(connection.query_row(
            "SELECT count(*) FROM person WHERE admin AND active
             AND (password IS NOT NULL OR (? AND email IS NOT NULL) OR NOT ?)",
            [self.google, signs_in],
            |row| row.get(0),
        )?)
    }

    /// Refuses an account nobody could sign in to.
    fn enterable(&self, password: bool, email: bool) -> Result<(), Refused> {
        match (password, email, self.google) {
            (true, _, _) | (_, true, true) => Ok(()),
            (false, _, true) => Err(Refused::Invalid(
                "a person signs in with a password, with Google by their email, or with either: \
                 give one of the two"
                    .to_owned(),
            )),
            (false, _, false) => Err(Refused::Invalid(
                "this server does not sign people in with Google, so a person needs a password"
                    .to_owned(),
            )),
        }
    }

    /// Makes the first account, an admin. Refused once an admin can sign in.
    pub fn first(&self, username: &str, name: &str, password: &str) -> Result<Entered, Refused> {
        let person = drafted(username, name, None, true)?;
        let hash = hashed(password)?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        if self.admins(&transaction, true)? != 0 {
            return Err(Refused::SetUp);
        }
        let person = insert(&transaction, person, Some(&hash))?;
        transaction.commit()?;
        Ok(Entered {
            person,
            mark: mark(&hash),
        })
    }

    pub fn add(&self, new: New) -> Result<Person, Refused> {
        let person = drafted(&new.username, &new.name, new.email.as_deref(), new.admin)?;
        self.enterable(new.password.is_some(), person.email.is_some())?;
        let hash = new.password.as_deref().map(hashed).transpose()?;
        insert(&self.connection(), person, hash.as_deref())
    }

    /// The account `spool dev` works as: an admin nobody can sign in as with
    /// a password. Made the first time it is asked for.
    pub fn dev(&self) -> Result<Person, Refused> {
        let connection = self.connection();
        if let Some(person) = find(&connection, "username", "dev")? {
            return Ok(person);
        }
        insert(&connection, drafted("dev", "Dev", None, true)?, None)
    }

    pub fn get(&self, id: &str) -> Result<Option<Person>, Refused> {
        find(&self.connection(), "id", id)
    }

    /// Everyone, in the order they were added.
    pub fn list(&self) -> Result<Vec<Person>, Refused> {
        let connection = self.connection();
        let mut statement = connection.prepare(&format!("{PEOPLE} ORDER BY created, username"))?;
        let people = statement
            .query_map([], person)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(people)
    }

    /// Changes a person. Turning an account off, or changing its password,
    /// ends its sessions. Another email is another Google account's: the
    /// sessions that began with the one before end.
    pub fn change(&self, id: &str, change: Change) -> Result<Person, Refused> {
        let name = change.name.as_deref().map(named).transpose()?;
        let hash = change.password.as_deref().map(hashed).transpose()?;
        let email = change.email.as_deref().map(addressed).transpose()?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let before = find(&transaction, "id", id)?.ok_or(Refused::Unknown)?;
        // Once an admin can sign in, one always can. Until then `dev` is the
        // admin, and stays one.
        let signs_in = self.admins(&transaction, true)? != 0;
        let after = Person {
            name: name.unwrap_or(before.name.clone()),
            admin: change.admin.unwrap_or(before.admin),
            active: change.active.unwrap_or(before.active),
            email: email.unwrap_or(before.email.clone()),
            password: before.password || hash.is_some(),
            ..before.clone()
        };
        if after.email != before.email {
            if after.email.is_none() && !after.password {
                return Err(Refused::Invalid(
                    "this person signs in with Google only, and could not without an email: \
                     give them a password as well, or turn the account off"
                        .to_owned(),
                ));
            }
            if let Some(email) = &after.email {
                let theirs: Option<String> = transaction
                    .query_row("SELECT id FROM person WHERE email = ?", [email], |row| {
                        row.get(0)
                    })
                    .optional()?;
                if theirs.is_some() {
                    return Err(Refused::EmailTaken(email.clone()));
                }
            }
            transaction.execute(
                "UPDATE person SET email = ?, google = NULL, bound = NULL WHERE id = ?",
                params![after.email, id],
            )?;
            // Those would begin nothing again, and are not kept.
            transaction.execute(
                &format!(
                    "DELETE FROM session WHERE json_extract(data, '$.{PERSON}') = ? \
                     AND json_extract(data, '$.{GOOGLE}') IS NOT NULL"
                ),
                [id],
            )?;
            grants::end_approvals(&transaction, id, Some(GOOGLE))?;
        }
        transaction.execute(
            "UPDATE person SET name = ?, admin = ?, active = ?, password = coalesce(?, password) WHERE id = ?",
            params![after.name, after.admin, after.active, hash, id],
        )?;
        if self.admins(&transaction, signs_in)? == 0 {
            return Err(Refused::LastAdmin);
        }
        if hash.is_some() || !after.active {
            end_sessions(&transaction, id)?;
        }
        transaction.commit()?;
        Ok(after)
    }
}

/// A person, and after what a [`Person`] has, the hash of their password.
pub(super) const PEOPLE: &str = "SELECT id, username, name, admin, active, email, password IS NOT NULL, password, bound FROM person";

/// Where the hash of the password is in a row of [`PEOPLE`].
pub(super) const PASSWORD: usize = 7;

/// Where the mark of the account's Google account is.
pub(super) const BOUND: usize = 8;

pub(super) fn person(row: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
    Ok(Person {
        id: row.get(0)?,
        username: row.get(1)?,
        name: row.get(2)?,
        admin: row.get(3)?,
        active: row.get(4)?,
        email: row.get(5)?,
        password: row.get(6)?,
    })
}

fn find(connection: &Connection, column: &str, value: &str) -> Result<Option<Person>, Refused> {
    Ok(connection
        .query_row(&format!("{PEOPLE} WHERE {column} = ?"), [value], person)
        .optional()?)
}

/// Stores a new account, with the hash of its password if it has one.
fn insert(connection: &Connection, person: Person, hash: Option<&str>) -> Result<Person, Refused> {
    let person = Person {
        password: hash.is_some(),
        ..person
    };
    if let Some(email) = &person.email
        && find(connection, "email", email)?.is_some()
    {
        return Err(Refused::EmailTaken(email.clone()));
    }
    let inserted = connection.execute(
        "INSERT INTO person (id, username, name, password, admin, active, email)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (username) DO NOTHING",
        params![
            person.id,
            person.username,
            person.name,
            hash,
            person.admin,
            person.active,
            person.email
        ],
    )?;
    match inserted {
        0 => Err(Refused::Taken(person.username)),
        _ => Ok(person),
    }
}

/// A new account as it would be stored, with a new ID.
fn drafted(
    username: &str,
    name: &str,
    email: Option<&str>,
    admin: bool,
) -> Result<Person, Refused> {
    let username = username.trim().to_lowercase();
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-@".contains(c);
    if username.is_empty() || username.len() > MAX_USERNAME || !username.chars().all(plain) {
        return Err(Refused::Invalid(
            "a username is 1 to 64 letters, digits, `.`, `_`, `-` or `@`".to_owned(),
        ));
    }
    Ok(Person {
        id: Uuid::now_v7().to_string(),
        username,
        name: named(name)?,
        admin,
        active: true,
        email: email.map(addressed).transpose()?.flatten(),
        password: false,
    })
}

/// An email as it is stored, or none if it is empty. It is compared with
/// what Google says, not written to: it has to look like an address and no
/// more.
fn addressed(email: &str) -> Result<Option<String>, Refused> {
    let email = email.trim().to_lowercase();
    if email.is_empty() {
        return Ok(None);
    }
    let parts = email.split_once('@');
    let whole = parts
        .is_some_and(|(name, host)| !name.is_empty() && !host.is_empty() && !host.contains('@'));
    if !whole
        || email.len() > MAX_EMAIL
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Refused::Invalid(
            "an email is an address such as `ada@example.com`".to_owned(),
        ));
    }
    Ok(Some(email))
}

fn named(name: &str) -> Result<String, Refused> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(Refused::Invalid("a name is 1 to 200 characters".to_owned()));
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::super::testing::{PASSWORD, email, new, with_email};
    use super::*;

    #[test]
    fn an_account_is_one_somebody_can_sign_in_to() {
        let plain = Accounts::in_memory();
        let refused = plain.add(with_email("grace", "grace@example.com"));
        assert!(matches!(refused, Err(Refused::Invalid(_))), "{refused:?}");
        // With a password as well, the email waits for Google.
        let both = New {
            email: Some("grace@example.com".to_owned()),
            ..new("grace", "Grace", PASSWORD)
        };
        assert!(plain.add(both.clone()).is_ok());

        let accounts = Accounts::in_memory().with_google(true);
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        let neither = New {
            username: "bob".to_owned(),
            name: "Bob".to_owned(),
            ..New::default()
        };
        let refused = accounts.add(neither);
        assert!(matches!(refused, Err(Refused::Invalid(_))), "{refused:?}");
        for wrong in [
            "grace",
            "@example.com",
            "grace@",
            "a b@example.com",
            "a@b@c",
        ] {
            let refused = accounts.add(with_email("grace", wrong));
            assert!(matches!(refused, Err(Refused::Invalid(_))), "{wrong}");
        }
        let grace = accounts.add(both).unwrap();
        let hopper = accounts
            .add(with_email("hopper", "hopper@example.com"))
            .unwrap();
        let taken = Refused::EmailTaken("grace@example.com".to_owned());
        assert_eq!(
            accounts.add(with_email("other", "Grace@example.com")),
            Err(taken.clone())
        );
        assert_eq!(
            accounts.change(&hopper.id, email("grace@example.com")),
            Err(taken)
        );

        // Whoever has a password can do without the email. Who has not, cannot.
        assert_eq!(accounts.change(&grace.id, email("")).unwrap().email, None);
        let refused = accounts.change(&hopper.id, email(" "));
        assert!(matches!(refused, Err(Refused::Invalid(_))), "{refused:?}");
        let both = Change {
            password: Some(PASSWORD.to_owned()),
            ..email("")
        };
        assert_eq!(accounts.change(&hopper.id, both).unwrap().email, None);
        assert!(accounts.verify("hopper", PASSWORD).unwrap().is_some());
        // An email of one's own is left as it is.
        let ada = accounts.change(&ada.id, email("ada@example.com")).unwrap();
        assert_eq!(accounts.change(&ada.id, email("ADA@example.com")), Ok(ada));
    }

    #[test]
    fn an_admin_who_signs_in_with_google_is_one_only_where_google_signs_people_in() {
        let accounts = Accounts::in_memory().with_google(true);
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        accounts
            .add(New {
                admin: true,
                ..with_email("grace", "grace@example.com")
            })
            .unwrap();
        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        // Grace is left to manage people.
        assert!(!accounts.change(&ada.id, off.clone()).unwrap().active);
        assert!(!accounts.needs_setup().unwrap());

        // The same file under a server that has no Google: nobody gets in,
        // so it prints the link that makes a first account.
        let plain = accounts.clone().with_google(false);
        assert!(plain.needs_setup().unwrap());
        assert!(plain.first("eve", "Eve", PASSWORD).is_ok());
    }

    #[test]
    fn the_first_account_is_an_admin_and_there_is_only_one_first() {
        let accounts = Accounts::in_memory();
        assert!(accounts.needs_setup().unwrap());
        let ada = accounts
            .first("Ada", " Ada Lovelace ", PASSWORD)
            .unwrap()
            .person;
        assert!(ada.admin && ada.active);
        assert_eq!(
            (ada.username.as_str(), ada.name.as_str()),
            ("ada", "Ada Lovelace")
        );
        assert_eq!(accounts.first("eve", "Eve", PASSWORD), Err(Refused::SetUp));
        assert_eq!(accounts.list().unwrap(), [ada]);
    }

    #[test]
    fn what_cannot_be_an_account_is_refused() {
        let accounts = Accounts::in_memory();
        accounts.first("ada", "Ada", PASSWORD).unwrap();
        for (username, name, password) in [
            ("", "Ada", PASSWORD),
            ("a b", "Ada", PASSWORD),
            ("bob", " ", PASSWORD),
            ("bob", "Bob", "short"),
        ] {
            let refused = accounts.add(new(username, name, password)).unwrap_err();
            assert!(matches!(refused, Refused::Invalid(_)), "{refused:?}");
        }
        assert_eq!(
            accounts.add(new("ADA", "Other", PASSWORD)),
            Err(Refused::Taken("ada".to_owned()))
        );
    }

    #[test]
    fn the_dev_account_has_no_password() {
        let accounts = Accounts::in_memory();
        let dev = accounts.dev().unwrap();
        assert_eq!(accounts.dev().unwrap(), dev);
        assert!(dev.admin);
        assert_eq!(accounts.verify("dev", "").unwrap(), None);
    }

    #[test]
    fn the_dev_account_does_not_stand_in_for_the_first_one() {
        let accounts = Accounts::in_memory();
        let dev = accounts.dev().unwrap();
        // Someone `spool dev` added signs in, and manages nobody.
        accounts.add(new("bob", "Bob", PASSWORD)).unwrap();
        assert!(accounts.needs_setup().unwrap());
        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        // Until someone signs in to manage people, `dev` is who does.
        assert_eq!(
            accounts.change(&dev.id, off.clone()),
            Err(Refused::LastAdmin)
        );

        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap().person;
        assert!(!accounts.needs_setup().unwrap());
        assert_eq!(accounts.get(&dev.id).unwrap(), Some(dev.clone()));
        // `dev` is an admin nobody signs in as: it cannot be the one left.
        let plain = Change {
            admin: Some(false),
            ..Change::default()
        };
        assert_eq!(accounts.change(&ada.id, plain), Err(Refused::LastAdmin));
        assert_eq!(
            accounts.change(&ada.id, off.clone()),
            Err(Refused::LastAdmin)
        );
        assert!(!accounts.change(&dev.id, off).unwrap().active);
    }
}
