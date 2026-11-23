import { useState } from "react";
import type { ReactNode } from "react";

import { Act, Done, Mistake } from "../act.tsx";
import type { ChangePersonRequest, Person } from "../api.ts";
import { Code, Load, Page } from "../components/layout.tsx";
import { Table } from "../components/table.tsx";
import { Word } from "../components/refs.tsx";
import { Flag, Labelled } from "../components/forms.tsx";
import { Input } from "spool-ui/components/input";
import { useStudio } from "../hooks.ts";
import { UserPlusIcon } from "../icons.tsx";
import { keys, stale, useRead } from "../queries.ts";

/** The fewest characters the server takes as a password. */
const MIN_PASSWORD = 12;

/** A field for a password that is being set, not asked for. */
export function NewPassword({
  label = "Password",
  optional = false,
  empty = "Unchanged",
}: {
  label?: string;
  optional?: boolean;
  /** What leaving it empty comes to, when it may be left empty. */
  empty?: string;
}) {
  return (
    <Labelled label={label}>
      <Input
        name="password"
        type="password"
        autoComplete="new-password"
        minLength={MIN_PASSWORD}
        required={!optional}
        placeholder={optional ? empty : `${MIN_PASSWORD} characters or more`}
      />
    </Labelled>
  );
}

/** The email of the Google account someone signs in with. */
function Email({ of }: { of?: Person }) {
  return (
    <Labelled label="Email" hint="Of the Google account they sign in with.">
      <Input
        name="email"
        type="email"
        defaultValue={of?.email ?? ""}
        autoComplete="off"
        spellCheck={false}
        autoCapitalize="none"
        placeholder="None: signs in with a password"
      />
    </Labelled>
  );
}

/** The ways an account can be signed in to. */
function signsInWith(person: Person): string {
  if (person.password && person.email) return "Password or Google";
  if (person.password) return "Password";
  return person.email ? "Google" : "-";
}

/** What was typed in a field, without the space around it. */
export function typed(entered: FormData, field: string): string {
  return String(entered.get(field) ?? "").trim();
}

/** A password as it was typed: the space around it is part of it. */
export function password(entered: FormData, field = "password"): string {
  return String(entered.get(field) ?? "");
}

export function People() {
  const { me, google } = useStudio();
  const [done, setDone] = useState<ReactNode>(null);
  const people = useRead(keys.people.list, (client) => client.people());
  return (
    <Page title="People" actions={me.admin ? <Add done={setDone} /> : null}>
      <Done>{done}</Done>
      <Load state={people}>
        {(rows) => (
          <Table
            columns={[
              { title: "Name", value: (person) => person.name, cell: (person) => person.name },
              {
                title: "Username",
                value: (person) => person.username,
                cell: (person) => <Code>{person.username}</Code>,
              },
              // Where nobody signs in with Google, everyone has a password and no more.
              ...(google || rows.some((person) => person.email)
                ? [
                    {
                      title: "Email",
                      value: (person: Person) => person.email,
                      cell: (person: Person) => person.email ?? "-",
                    },
                    { title: "Signs in with", value: signsInWith, cell: signsInWith },
                  ]
                : []),
              {
                title: "Role",
                value: (person) => (person.admin ? "admin" : null),
                cell: (person) => (person.admin ? <Word value="admin" label="Admin" tone="plain" /> : "-"),
              },
              {
                title: "Account",
                value: (person) => (person.active ? "on" : "off"),
                first: "desc",
                cell: (person) =>
                  person.active ? (
                    <Word value="on" label="On" tone="good" />
                  ) : (
                    <Word value="off" label="Off" tone="bad" />
                  ),
              },
              ...(me.admin
                ? [
                    {
                      title: "",
                      cell: (person: Person) => (
                        <Edit
                          key={`${person.admin}-${person.active}-${person.email}-${person.password}`}
                          person={person}
                          done={setDone}
                        />
                      ),
                    },
                  ]
                : []),
            ]}
            rows={rows}
            initial={{ by: "Name", order: "asc" }}
            rowKey={(person) => person.id}
            empty="No accounts."
            label="People"
          />
        )}
      </Load>
    </Page>
  );
}

function Add({ done }: { done: (said: ReactNode) => void }) {
  const { google } = useStudio();
  const [admin, setAdmin] = useState(false);
  return (
    <Act
      stale={stale.people}
      label="Add person"
      variant="default"
      icon={<UserPlusIcon />}
      act={async (client, { entered }) => {
        const email = typed(entered, "email");
        const secret = password(entered);
        if (email === "" && secret === "") throw new Mistake("Give an email, a password, or both.");
        const person = await client.addPerson({
          username: typed(entered, "username"),
          name: typed(entered, "name"),
          ...(email === "" ? {} : { email }),
          ...(secret === "" ? {} : { password: secret }),
          admin,
        });
        setAdmin(false);
        return `Added ${person.name}.`;
      }}
      done={done}
    >
      <Labelled label="Name">
        <Input name="name" autoComplete="off" required />
      </Labelled>
      <Labelled label="Username">
        <Input name="username" autoComplete="off" spellCheck={false} autoCapitalize="none" required />
      </Labelled>
      {google ? <Email /> : null}
      <NewPassword optional={google} empty="None: signs in with Google" />
      <Flag label="Admin" checked={admin} onChange={setAdmin} />
    </Act>
  );
}

function Edit({ person, done }: { person: Person; done: (said: ReactNode) => void }) {
  const { google } = useStudio();
  const emailed = google || Boolean(person.email);
  const [admin, setAdmin] = useState(person.admin);
  const [active, setActive] = useState(person.active);
  return (
    <Act
      stale={stale.people}
      label="Edit"
      title={`Edit ${person.name}`}
      confirm="Save"
      size="sm"
      act={async (client, { entered }) => {
        const change: ChangePersonRequest = {};
        const name = typed(entered, "name");
        if (name !== person.name) change.name = name;
        if (password(entered) !== "") change.password = password(entered);
        if (emailed && typed(entered, "email").toLowerCase() !== (person.email ?? "")) {
          change.email = typed(entered, "email");
        }
        if (admin !== person.admin) change.admin = admin;
        if (active !== person.active) change.active = active;
        if (Object.keys(change).length === 0) throw new Mistake("Nothing was changed.");
        const changed = await client.changePerson(person.id, change);
        return `Changed ${changed.name}.`;
      }}
      done={done}
    >
      <Labelled label="Name">
        <Input name="name" defaultValue={person.name} autoComplete="off" required />
      </Labelled>
      {emailed ? <Email of={person} /> : null}
      <NewPassword label="New password" optional empty={person.password ? "Unchanged" : "None"} />
      <Flag label="Admin" checked={admin} onChange={setAdmin} />
      <Flag label="Can sign in" checked={active} onChange={setActive} />
    </Act>
  );
}
