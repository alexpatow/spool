/**
 * The identity IDs the pages show for the companies of the sample project.
 *
 * An identity's ID is minted when the identity is made, so every run of the
 * samples prints others. `check-samples.ts --update` writes these in their
 * place, for the identity that holds the record named, so that each page shows
 * the same ID for the same company. The README and the front page
 * (`src/lib/sample.ts`) show the same ones.
 *
 * A company these do not name is shown under the ID that was printed.
 */
export const shownIds: Record<string, string> = {
  "crm:c-1": "01a1efc6-41cd-775d-8758-e7300637659a",
  "crm:c-4": "01a1efc6-41cd-775d-8758-e733aa086f7d",
  "crm:c-5": "01a1efc6-41cd-775d-8758-e7329447fd9a",
  "crm:c-6": "01a1efc6-41cd-775d-8758-e731c61371cb",
  "crm:c-8": "01a1efc6-42c0-7776-b736-83fde0cb4498",
  "notes:notes-1.txt[179-200]": "01a1efc6-43a9-7403-b9f6-1f8ee76184ed",
};
