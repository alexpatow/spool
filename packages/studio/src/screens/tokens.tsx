import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ReactNode } from "react";

import { Act, Done } from "../act.tsx";
import type { Token } from "../api.ts";
import { Code, Labelled, Load, Notice, Page, Table, When } from "../components.tsx";
import { CopyButton } from "../components/ui/copy-button";
import { Input } from "../components/ui/input";
import { useLoad, useStudio } from "../hooks.ts";
import { PlusIcon } from "../icons.tsx";
import { colors, radius, spacing, typography } from "../styles/shelf/tokens.stylex";
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
  const tokens = useLoad((client) => client.tokens());
  return (
    <Page title="Tokens" about={ABOUT} actions={<Add done={setDone} />}>
      <Done>{done}</Done>
      <Load state={tokens}>
        {(rows) => (
          <Table
            columns={[
              { title: "Token", cell: (token) => <Code>{token.name}</Code> },
              { title: "In the log", cell: (token) => <Code>agent:{token.name}</Code> },
              {
                title: "Made",
                cell: (token) => (
                  <>
                    <When at={token.created_at} /> by{" "}
                    {token.created_by ? <Who actor={`user:${token.created_by}`} /> : <Code>spool token add</Code>}
                  </>
                ),
              },
              {
                title: "Last used",
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
    </Page>
  );
}

function Add({ done }: { done: (said: ReactNode) => void }) {
  return (
    <Act
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
