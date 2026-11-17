// Invented company records with a known truth, in any number.
//
// The records have the columns of `benchmarks/identity/companies-small`, so
// that project's features and rules read them: a CRM that sends name,
// website, country, city and sometimes a registry number, and a registry
// that sends number, country, legal name, trading name and city.
//
// What makes them hard is what makes that fixture hard, at a rate:
//
//   aliases           a company's CRM rows spell its name with and without its legal form
//   trading names     the registry has a legal name and a trading name, and the CRM uses the trading name
//   a domain change   a later row of the same CRM record has another website
//   subsidiaries      a company shares its parent's domain and differs in registry number
//   shared names      unrelated companies have one name, in one country or in two
//   few attributes    a CRM row that is only a name
//   typing errors     a CRM row whose name has one letter wrong
//
// The same seed gives the same records.

/** A generator of numbers in [0, 1) that the seed decides (mulberry32). */
export function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface Row {
  [column: string]: string;
}

export interface Batch {
  crm: Row[];
  registry: Row[];
}

export interface Generated {
  batches: Batch[];
  /** The true entity of each record, by source and key. */
  truth: Array<{ source: string; key: string; entity: string }>;
  companies: number;
}

export interface Shape {
  /** How many true companies there are. */
  companies: number;
  seed: number;
  /** How many batches the records arrive in. The first holds most of them. */
  batches?: number;
}

const SYLLABLES =
  "al an ar bel bor bri cal cor dan del dor el fen gar hal hol ib jor kel kor lan lar len mar mel mor nor ol or pen quil ran rid sal sel sor tal ten tor ul val ven wes wil yar zen".split(
    " ",
  );
const ENDINGS = "field ford wick stead mere ham by ton well moor gate bridge side crest view land".split(" ");
const TRADES =
  "Logistics|Tools|Analytics|Foods|Supplies|Freight|Systems|Holdings|Partners|Labs|Energy|Textiles|Marine|Robotics|Capital|Media|Pharma|Mining|Brewing|Aviation|Software|Consulting|Timber|Optics|Foundry|Dairy|Motors|Cables|Glass|Paper".split(
    "|",
  );
const FORMS = ["Ltd", "Limited", "LLC", "Inc", "Corp", "PLC", "Co."];
const COUNTRIES = ["GB", "GB", "GB", "US", "US", "DE", "FR", "NL", "SE", "IE", "CA", "AU"];
const CITIES = "Leeds Sheffield Bristol Austin Denver Hamburg Lyon Utrecht Malmo Cork Calgary Perth".split(" ");

const CRM_COLUMNS = ["id", "name", "website", "country", "city", "registry_number"];
const REGISTRY_COLUMNS = ["number", "country", "legal_name", "trading_name", "city"];

export function generate({ companies, seed, batches = 2 }: Shape): Generated {
  const next = random(seed);
  const pick = <T>(from: T[]): T => from[Math.floor(next() * from.length)]!;
  const chance = (share: number) => next() < share;
  const title = (word: string) => word[0]!.toUpperCase() + word.slice(1);
  const invented = () => {
    const parts = 2 + Math.floor(next() * 2);
    let word = "";
    for (let part = 0; part < parts; part++) word += pick(SYLLABLES);
    return title(word + pick(ENDINGS));
  };

  const out: Generated = {
    batches: Array.from({ length: batches }, () => ({ crm: [], registry: [] })),
    truth: [],
    companies,
  };
  // The first batch holds seven tenths of the companies, and the others
  // share the rest.
  const batchOf = (company: number) => {
    const share = company / companies;
    if (batches === 1 || share < 0.7) return 0;
    return Math.min(batches - 1, 1 + Math.floor(((share - 0.7) / 0.3) * (batches - 1)));
  };

  let crmKey = 0;
  const earlier: Array<{ name: string; trade: string; country: string; domain: string | null; entity: string }> = [];
  for (let company = 0; company < companies; company++) {
    const entity = `e${company}`;
    const batch = out.batches[batchOf(company)]!;
    let first = invented();
    let trade = pick(TRADES);
    let country = pick(COUNTRIES);
    let domain: string | null = null;
    const kin = earlier.length > 0 && chance(0.04) ? earlier[Math.floor(next() * earlier.length)]! : undefined;
    const draw = next();
    if (kin && draw < 0.5) {
      // A subsidiary: the parent's first name and domain, another trade.
      first = kin.name;
      country = kin.country;
      domain = kin.domain;
      while (trade === kin.trade) trade = pick(TRADES);
    } else if (kin && draw < 0.8) {
      // The same name in another country.
      first = kin.name;
      trade = kin.trade;
      while (country === kin.country) country = pick(COUNTRIES);
    } else if (kin) {
      // The same name in the same country: only the registry tells them apart.
      first = kin.name;
      trade = kin.trade;
      country = kin.country;
    }
    const name = `${first} ${trade}`;
    const slug = `${first}${trade}`.toLowerCase();
    if (domain === null && chance(0.75)) domain = `${slug}${company}.example`;
    const city = pick(CITIES);
    const number = `X${String(company).padStart(8, "0")}`;
    const trading = chance(0.08) ? `${invented()} ${pick(TRADES)}` : null;
    earlier.push({ name: first, trade, country, domain, entity });
    if (earlier.length > 50_000) earlier.splice(0, 25_000);

    const registered = chance(0.85);
    if (registered) {
      batch.registry.push({
        number,
        country,
        legal_name: `${name} ${pick(FORMS)}`.toUpperCase().replace("CO.", "COMPANY"),
        trading_name: trading ?? "",
        city,
      });
      out.truth.push({ source: "registry", key: number, entity });
    }

    const records = registered ? (chance(0.6) ? 1 : chance(0.7) ? 2 : chance(0.5) ? 3 : 0) : 1 + Math.floor(next() * 2);
    for (let record = 0; record < records; record++) {
      let shown = trading !== null && chance(0.6) ? trading : name;
      if (chance(0.5)) shown = `${shown} ${pick(FORMS)}`;
      if (chance(0.1)) shown = shown.toUpperCase();
      if (chance(0.04)) {
        const at = 1 + Math.floor(next() * (shown.length - 2));
        shown = shown.slice(0, at) + "x" + shown.slice(at + 1);
      }
      const bare = chance(0.05);
      const website =
        domain === null || bare || chance(0.2)
          ? ""
          : pick(["https://www.", "http://www.", "https://", ""]) + domain + pick(["", "", "/", "/contact"]);
      const row: Row = {
        id: `c-${crmKey++}`,
        name: shown,
        website,
        country: bare ? "" : country,
        city: bare ? "" : city,
        registry_number: !bare && registered && chance(0.3) ? number : "",
      };
      batch.crm.push(row);
      out.truth.push({ source: "crm", key: row.id!, entity });

      // A later batch sends the row again, as it was or with a new website.
      if (batches > 1 && batchOf(company) === 0 && chance(0.06)) {
        const again = { ...row };
        if (chance(0.4) && domain !== null) again.website = `https://${slug}-group${company}.example`;
        out.batches[1 + Math.floor(next() * (batches - 1))]!.crm.push(again);
      }
    }
  }
  return out;
}

const quoted = (value: string) => (/[",\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value);

export function csv(columns: string[], rows: Row[]): string {
  const lines = [columns.join(",")];
  for (const row of rows) lines.push(columns.map((column) => quoted(row[column] ?? "")).join(","));
  return lines.join("\n") + "\n";
}

export const crmCsv = (rows: Row[]) => csv(CRM_COLUMNS, rows);
export const registryCsv = (rows: Row[]) => csv(REGISTRY_COLUMNS, rows);

export function truthCsv(truth: Generated["truth"]): string {
  const lines = ["source,source_key,true_entity"];
  for (const { source, key, entity } of truth) lines.push(`${source},${key},${entity}`);
  return lines.join("\n") + "\n";
}

/** How many records a generated set holds. */
export function records(generated: Generated): number {
  return generated.batches.reduce((sum, batch) => sum + batch.crm.length + batch.registry.length, 0);
}
