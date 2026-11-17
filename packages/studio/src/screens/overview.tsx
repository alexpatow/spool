import type { Info } from "../api.ts";
import { Fields, LinkTo, Load, Page, Ref, Section, Table } from "../components.tsx";
import { useLoad } from "../hooks.ts";
import { href } from "../router.ts";

export function Overview() {
  const info = useLoad((client) => client.info());
  return (
    <Page title="Overview" about="What this server holds.">
      <Load state={info}>{(info) => <Store info={info} />}</Load>
    </Page>
  );
}

function Store({ info }: { info: Info }) {
  const { store } = info;
  return (
    <>
      <Fields
        rows={[
          ["API", `${info.api}, engine ${store.engine_version}, store format ${store.format_version}`],
          ["Manifest", store.manifest_hash ? <code>{store.manifest_hash}</code> : "none applied"],
          ["Head", <LinkTo path={["log"]}>log position {store.head}</LinkTo>],
          ["Documents", <LinkTo path={["documents"]}>{store.documents ?? 0}</LinkTo>],
        ]}
      />
      {store.entity_types.length === 0 ? <p className="quiet">The project declares no entity type.</p> : null}
      {store.entity_types.map((type) => (
        <Section key={type.name} title={`Entity type ${type.name}`}>
          <div className="cards">
            <a className="card" href={href(["observations"], { type: type.name })}>
              <strong>{type.observations}</strong>
              <span>{type.observations === 1 ? "observation" : "observations"}</span>
            </a>
            <a className="card" href={href(["identities"], { type: type.name })}>
              <strong>{type.identities}</strong>
              <span>{type.identities === 1 ? "identity" : "identities"}</span>
            </a>
            <a
              className={`card ${type.unassigned > 0 ? "waits" : ""}`}
              href={href(["observations"], { type: type.name, unassigned: true })}
            >
              <strong>{type.unassigned}</strong>
              <span>without an identity</span>
            </a>
            <a
              className={`card ${type.open_review_items > 0 ? "waits" : ""}`}
              href={href(["review"], { type: type.name, status: "open" })}
            >
              <strong>{type.open_review_items}</strong>
              <span>{type.open_review_items === 1 ? "review item open" : "review items open"}</span>
            </a>
          </div>
          <Fields
            rows={[
              [
                "Draft",
                type.open_draft ? (
                  <>
                    <Ref kind="draft" id={type.open_draft} /> waits for a decision
                  </>
                ) : null,
              ],
              [
                "Features",
                type.features.length > 0
                  ? type.features.map((column, index) => (
                      <span key={column.name}>
                        {index > 0 ? ", " : ""}
                        <code>{column.name}</code> <span className="quiet">{column.data_type.toLowerCase()}</span>
                      </span>
                    ))
                  : null,
              ],
              [
                "Strategies",
                <>
                  <code>rules</code>
                  {(type.strategies ?? []).map((strategy) => (
                    <span key={strategy.name}>
                      , <code>{strategy.name}</code> <span className="quiet">{strategy.runtime}</span>
                    </span>
                  ))}
                </>,
              ],
              ["Declaration", `version ${type.declaration_version}`],
            ]}
          />
        </Section>
      ))}
      {(store.extractors ?? []).length > 0 ? (
        <Section title="Extractors">
          <Table
            columns={[
              { title: "Extractor", cell: (extractor) => <code>{extractor.name}</code> },
              { title: "Runs with", cell: (extractor) => extractor.runtime },
              { title: "File", cell: (extractor) => <code>{extractor.file}</code> },
            ]}
            rows={store.extractors ?? []}
            rowKey={(extractor) => extractor.name}
            empty="The project has no extractor."
          />
        </Section>
      ) : null}
    </>
  );
}
