import { useState } from "react";
import type { ReactNode } from "react";

import { Act, Done, Mistake } from "../act.tsx";
import type { ChangePersonRequest, Person } from "../api.ts";
import { Code, Flag, Labelled, Load, Page, Table, Word } from "../components.tsx";
import { Input } from "../components/ui/input";
import { useLoad, useStudio } from "../hooks.ts";
import { UserPlusIcon } from "../icons.tsx";

/** The fewest characters the server takes as a password. */
export const MIN_PASSWORD = 12;

/** A field for a password that is being set, not asked for. */
export function NewPassword({ label = "Password", optional = false }: { label?: string; optional?: boolean }) {
  return (
    <Labelled label={label}>
      <Input
        name="password"
        type="password"
        autoComplete="new-password"
        minLength={MIN_PASSWORD}
        required={!optional}
        placeholder={optional ? "Unchanged" : `${MIN_PASSWORD} characters or more`}
      />
    </Labelled>
  );
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
  const { me } = useStudio();
  const [done, setDone] = useState<ReactNode>(null);
  const people = useLoad((client) => client.people());
  return (
    <Page title="People" actions={me.admin ? <Add done={setDone} /> : null}>
      <Done>{done}</Done>
      <Load state={people}>
        {(rows) => (
          <Table
            columns={[
              { title: "Name", cell: (person) => person.name },
              { title: "Username", cell: (person) => <Code>{person.username}</Code> },
              {
                title: "Role",
                cell: (person) => (person.admin ? <Word value="admin" label="Admin" tone="plain" /> : "-"),
              },
              {
                title: "Account",
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
                        <Edit key={`${person.admin}-${person.active}`} person={person} done={setDone} />
                      ),
                    },
                  ]
                : []),
            ]}
            rows={rows}
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
  const [admin, setAdmin] = useState(false);
  return (
    <Act
      label="Add person"
      variant="default"
      icon={<UserPlusIcon />}
      act={async (client, { entered }) => {
        const person = await client.addPerson({
          username: typed(entered, "username"),
          name: typed(entered, "name"),
          password: password(entered),
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
      <NewPassword />
      <Flag label="Admin" checked={admin} onChange={setAdmin} />
    </Act>
  );
}

function Edit({ person, done }: { person: Person; done: (said: ReactNode) => void }) {
  const [admin, setAdmin] = useState(person.admin);
  const [active, setActive] = useState(person.active);
  return (
    <Act
      label="Edit"
      title={`Edit ${person.name}`}
      confirm="Save"
      size="sm"
      act={async (client, { entered }) => {
        const change: ChangePersonRequest = {};
        const name = typed(entered, "name");
        if (name !== person.name) change.name = name;
        if (password(entered) !== "") change.password = password(entered);
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
      <NewPassword label="New password" optional />
      <Flag label="Admin" checked={admin} onChange={setAdmin} />
      <Flag label="Can sign in" checked={active} onChange={setActive} />
    </Act>
  );
}
