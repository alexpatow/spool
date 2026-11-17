import type { ObservationRecord, ReviewRecord, ReviewStatus } from "../api.ts";
import { Fields, Filters, Load, Page, Ref, Section, Table, When, Word, usePair } from "../components.tsx";
import { found } from "../find.ts";
import { count, observationName } from "../format.ts";
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
