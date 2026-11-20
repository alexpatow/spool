import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, Done } from "../act.tsx";
import type { Grant, Token } from "../api.ts";
import { Code, Labelled, Load, Notice, Page, Section, Table, When } from "../components.tsx";
import { CopyButton } from "spool-ui/components/copy-button";
import { Input } from "spool-ui/components/input";
import { useStudio } from "../hooks.ts";
import { PlusIcon } from "../icons.tsx";
import { keys, stale, useRead } from "../queries.ts";
import { colors, radius, spacing, typography } from "spool-ui/tokens.stylex";
import { Who } from "../who.tsx";
import { typed } from "./people.tsx";

const ABOUT = "What an agent or a pipeline presents instead of signing in.";

export function Tokens() {
  const { me } = useStudio();
  if (!me.admin) {
    return (
      <Page title="Tokens" about={ABOUT}>
        <Notice>Only an admin makes and revokes tokens.</Notice>
      </Page>
    );
  }
  return <Managed />;
}

function Managed() {
  const [done, setDone] = useState<ReactNode>(null);
  const tokens = useRead(keys.tokens.list(), (client) => client.tokens());
  const grants = useRead(keys.grants.list(), (client) => client.grants());
  return (
    <Page title="Tokens" about={ABOUT} actions={<Add done={setDone} />}>
      <Done>{done}</Done>
      <Load state={tokens}>
        {(rows) => (
          <Table
            columns={[
              { title: "Token", value: (token) => token.name, cell: (token) => <Code>{token.name}</Code> },
              { title: "In the log", value: (token) => token.name, cell: (token) => <Code>agent:{token.name}</Code> },
              {
                title: "Made",
                value: (token) => token.created_at,
                first: "desc",
                cell: (token) => (
                  <>
                    <When at={token.created_at} /> by{" "}
                    {token.created_by ? <Who actor={`user:${token.created_by}`} /> : <Code>spool token add</Code>}
                  </>
                ),
              },
              {
                title: "Last used",
                value: (token) => token.last_used_at,
                first: "desc",
                cell: (token) => (token.last_used_at ? <When at={token.last_used_at} /> : "Never"),
              },
              { title: "", cell: (token: Token) => <Revoke token={token} done={setDone} /> },
            ]}
            rows={rows}
            rowKey={(token) => token.id}
            empty="No tokens. Nothing but a person who signs in can call this server."
            label="Tokens"
          />
        )}
      </Load>
      <Section
        title="Assistants"
        about={
          <>
            Assistants that someone approved with OAuth, such as Claude or ChatGPT on the web. Each works at{" "}
            <Code>/mcp</Code> as the person who approved it.
          </>
        }
      >
        <Load state={grants}>
          {(rows) => (
            <Table
              columns={[
                {
                  title: "Assistant",
                  value: (grant) => grant.client_name ?? "",
                  cell: (grant) => grant.client_name ?? "Unnamed",
                },
                {
                  title: "Returns to",
                  value: (grant) => host(grant.redirect_uri),
                  cell: (grant) => <Code>{host(grant.redirect_uri)}</Code>,
                },
                {
                  title: "Approved",
                  value: (grant) => grant.created_at,
                  first: "desc",
                  cell: (grant) => (
                    <>
                      <When at={grant.created_at} /> by <Who actor={`user:${grant.person}`} />
                    </>
                  ),
                },
                {
                  title: "Last used",
                  value: (grant) => grant.last_used_at,
                  first: "desc",
                  cell: (grant) => (grant.last_used_at ? <When at={grant.last_used_at} /> : "Never"),
                },
                { title: "", cell: (grant: Grant) => <EndGrant grant={grant} done={setDone} /> },
              ]}
              rows={rows}
              rowKey={(grant) => grant.id}
              empty="No assistant has been approved."
              label="Assistants"
            />
          )}
        </Load>
      </Section>
    </Page>
  );
}

function host(address: string): string {
  try {
    return new URL(address).host;
  } catch {
    return address;
  }
}

function EndGrant({ grant, done }: { grant: Grant; done: (said: ReactNode) => void }) {
  const name = grant.client_name ?? "the assistant";
  return (
    <Act
      stale={stale.grants}
      label="Revoke"
      title={`Revoke ${name}`}
      variant="destructive"
      size="sm"
      about="Its tokens stop working at once. To use this server again, it has to be approved again."
      act={async (client) => {
        await client.revokeGrant(grant.id);
        return <span>Revoked {name}.</span>;
      }}
      done={done}
    />
  );
}

function Add({ done }: { done: (said: ReactNode) => void }) {
  return (
    <Act
      stale={stale.tokens}
      label="Add token"
      variant="default"
      icon={<PlusIcon />}
      about="What it changes is recorded under its name."
      act={async (client, { entered }) => {
        const { token, secret } = await client.addToken(typed(entered, "name"));
        return <Made name={token.name} secret={secret} />;
      }}
      done={done}
    >
      <Labelled label="Name" hint="Letters, digits, `.`, `_` and `-`">
        <Input
          name="name"
          autoComplete="off"
          spellCheck={false}
          autoCapitalize="none"
          maxLength={64}
          pattern="[A-Za-z0-9._\-]+"
          placeholder="load"
          required
        />
      </Labelled>
    </Act>
  );
}

/** A token that was just made: the only time anyone can read it. */
function Made({ name, secret }: { name: string; secret: string }) {
  return (
    <>
      <span>
        Made the token <Code>{name}</Code>. Copy it now: it is not shown again.
      </span>
      <span data-part="secret" {...stylex.props(styles.secret)}>
        <code {...stylex.props(styles.shown)}>{secret}</code>
        <CopyButton text={secret} label="Copy token" showLabel />
      </span>
      <span>
        A caller sends it as <Code>Authorization: Bearer &lt;token&gt;</Code>.
      </span>
    </>
  );
}

function Revoke({ token, done }: { token: Token; done: (said: ReactNode) => void }) {
  return (
    <Act
      stale={stale.tokens}
      label="Revoke"
      title={`Revoke ${token.name}`}
      variant="destructive"
      size="sm"
      about={
        <>
          Whatever presents <Code>{token.name}</Code> is refused from now on. What it changed stays in the log.
        </>
      }
      act={async (client) => {
        const revoked = await client.revokeToken(token.id);
        return (
          <span>
            Revoked <Code>{revoked.name}</Code>.
          </span>
        );
      }}
      done={done}
    />
  );
}

const styles = stylex.create({
  secret: {
    borderColor: colors.border,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: 1,
    gap: spacing["2"],
    paddingBlock: spacing["1"],
    paddingInline: spacing["2"],
    alignItems: "center",
    alignSelf: "flex-start",
    backgroundColor: colors.background,
    color: colors.foreground,
    display: "flex",
    maxWidth: "100%",
  },
  shown: {
    fontFamily: typography.fontFamilyMono,
    fontSize: "0.8125rem",
    overflowWrap: "anywhere",
    userSelect: "all",
  },
});
