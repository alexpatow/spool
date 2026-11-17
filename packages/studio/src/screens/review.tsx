import { useState } from "react";
import type { ReactNode } from "react";

import { Act, At, Done, Mistake } from "../act.tsx";
import type { Op, ObservationRecord, ReviewRecord, ReviewStatus } from "../api.ts";
import { Fields, Filters, Load, Page, Ref, Section, Table, When, Word, usePair } from "../components.tsx";
import { found } from "../find.ts";
import { count, observationName, short } from "../format.ts";
import { useLoad } from "../hooks.ts";
import type { Route } from "../router.ts";

const STATUSES: ReviewStatus[] = ["open", "resolved", "superseded", "dismissed"];

export function ReviewItems({ route, types }: { route: Route; types: string[] }) {
  const { status, type } = route.query;
  const items = useLoad((client) =>
    client.reviewItems({ status: status as ReviewStatus | undefined, entity_type: type }),
  );
  return (
    <Page
      title="Review"
      about="What reconciling would not decide. The observations an item names have no identity while it is open."
    >
      <Filters
        path={["review"]}
        query={route.query}
        selects={[
          { name: "status", label: "Status", choices: STATUSES.map((value) => ({ value })) },
          { name: "type", label: "Entity type", choices: types.map((value) => ({ value })) },
        ]}
      />
      <Load state={items}>
        {(items) => (
          <Table
            columns={[
              { title: "Item", cell: (item) => <Ref kind="review" id={item.id} /> },
              { title: "Kind", cell: (item) => <Word value={item.kind} /> },
              { title: "Status", cell: (item) => <Word value={item.status} /> },
              { title: "Entity type", cell: (item) => item.entity_type },
              { title: "Holds", cell: (item) => count(item.observations.length, "observation") },
              { title: "Matched", cell: (item) => count(item.identities.length, "identity") },
              { title: "Raised by", cell: (item) => <Ref kind="run" id={item.run} label={`run ${item.run}`} /> },
            ]}
            rows={items}
            rowKey={(item) => item.id}
            empty="No review items."
          />
        )}
      </Load>
    </Page>
  );
}

interface Loaded {
  item: ReviewRecord;
  observations: ObservationRecord[];
}

export function ReviewItem({ id }: { id: string }) {
  const loaded = useLoad(async (client): Promise<Loaded> => {
    const item = await client.reviewItem(Number(id));
    const observations = await Promise.all(
      item.observations.map((observation) => found(client.observation(observation))),
    );
    return { item, observations: observations.filter((observation) => observation !== null) };
  });
  return (
    <Page title={`Review item ${id}`}>
      <Load state={loaded}>{(loaded) => <ReviewScreen {...loaded} />}</Load>
    </Page>
  );
}

function ReviewScreen({ item, observations }: Loaded) {
  const pair = usePair();
  return (
    <>
      <Fields
        rows={[
          ["Kind", <Word value={item.kind} />],
          ["Status", <Word value={item.status} />],
          ["Entity type", item.entity_type],
          ["Raised by", <Ref kind="run" id={item.run} label={`reconcile run ${item.run}`} />],
          [
            "Resolved at",
            item.resolved_seq ? (
              <Ref kind="log" id={item.resolved_seq} label={`log position ${item.resolved_seq}`} />
            ) : null,
          ],
          [
            "Raised again by",
            item.superseded_by ? (
              <Ref kind="run" id={item.superseded_by} label={`reconcile run ${item.superseded_by}`} />
            ) : null,
          ],
          [
            "Dismissed by",
            item.dismissed ? (
              <>
                <code>{item.dismissed.by}</code> at <When at={item.dismissed.at} />
                {item.dismissed.reason ? <span className="reason"> “{item.dismissed.reason}”</span> : null}
              </>
            ) : null,
          ],
        ]}
      />
      <Decide item={item} observations={observations} />
      <Section title="Observations left without an identity">
        <Table
          columns={[
            { title: "", cell: (observation) => pair.tick(observation.id) },
            {
              title: "Observation",
              cell: (observation) => (
                <Ref kind="observation" id={observation.id} label={observationName(observation)} />
              ),
            },
            { title: "ID", cell: (observation) => <Ref kind="observation" id={observation.id} /> },
            {
              title: "Identity now",
              cell: (observation) =>
                observation.identity ? (
                  <Ref kind="identity" id={observation.identity} />
                ) : (
                  <Word value="unassigned" label="none" />
                ),
            },
          ]}
          rows={observations}
          rowKey={(observation) => observation.id}
          empty="The item names no observation."
        />
        {observations.length > 1 ? pair.explain : null}
      </Section>
      {item.identities.length > 0 ? (
        <Section title="Identities they matched">
          <ul className="plain">
            {item.identities.map((identity) => (
              <li key={identity}>
                <Ref kind="identity" id={identity} label={identity} />
              </li>
            ))}
          </ul>
        </Section>
      ) : null}
    </>
  );
}

/** In a new identity, as a choice of where an observation goes. */
const NEW = "new";

/**
 * Settles an open item: its observations are placed, which resolves it once
 * each has an identity, or it is dismissed.
 */
function Decide({ item, observations }: Loaded) {
  const [done, setDone] = useState<ReactNode>(null);
  const open = item.status === "open";
  if (!open && done === null) return null;
  const waiting = observations.filter((observation) => !observation.identity);
  return (
    <Section title="Decide">
      <Done>{done}</Done>
      {open && waiting.length > 0 ? (
        <Act
          label={waiting.length === 1 ? "Place it" : "Place them"}
          who
          why="Reason"
          act={async (client, { actor, reason, entered }) => {
            const chosen = waiting.flatMap((observation) => {
              const to = String(entered.get(`to-${observation.id}`) ?? "");
              return to === "" ? [] : [{ observation: observation.id, to }];
            });
            if (chosen.length === 0) throw new Mistake("Choose where at least one of them goes.");
            const created = chosen.filter(({ to }) => to === NEW).length;
            const minted = created > 0 ? (await client.mintIds({ identities: created })).identities : [];
            const ops: Op[] = [];
            for (const { observation, to } of chosen) {
              let identity = to;
              if (to === NEW) {
                identity = minted.shift()!;
                ops.push({ op: "create_identity", entity_type: item.entity_type, identity });
              }
              ops.push({ op: "assign", observation, from: null, to: identity });
            }
            const committed = await client.commit({ actor, reason, ops });
            return (
              <>
                {count(chosen.length, "observation")} placed at <At committed={committed} />.
              </>
            );
          }}
          done={setDone}
        >
          {waiting.map((observation) => (
            <label key={observation.id}>
              Where {observationName(observation)} goes
              <select name={`to-${observation.id}`} defaultValue="">
                <option value="">nowhere yet</option>
                {item.identities.map((identity) => (
                  <option key={identity} value={identity}>
                    identity {short(identity)}
                  </option>
                ))}
                <option value={NEW}>a new identity of its own</option>
              </select>
            </label>
          ))}
        </Act>
      ) : null}
      {open ? (
        <Act
          label="Dismiss"
          who
          why="Reason"
          act={async (client, { actor, reason }) => {
            await client.dismissReview(item.id, { actor, reason });
            return "The item was dismissed. Later runs leave its observations alone.";
          }}
          done={setDone}
        />
      ) : null}
      {open && item.identities.length > 1 ? (
        <p className="quiet">
          If the identities they matched are one, merge one into the other on its screen, and then place the
          observations in it.
        </p>
      ) : null}
    </Section>
  );
}
