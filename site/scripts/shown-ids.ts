/**
 * The identity IDs the pages show for the companies of the sample project.
 *
 * An identity's ID is minted when the identity is made, so every run of the
 * samples prints others. `check-samples.ts --update` writes these in their
 * place, for the identity that holds the record named, so that each page shows
 * the same ID for the same company. The README shows the same ones.
 *
 * A company these do not name is shown under the ID that was printed.
 */
export const shownIds: Record<string, string> = {
  "crm:c-1": "01a1ef8e-5a8d-76d4-acb6-2bb5ad4d4ada",
  "crm:c-4": "01a1ef8e-5a8d-76d4-acb6-2bb802d5ca25",
  "crm:c-5": "01a1ef8e-5a8d-76d4-acb6-2bb793e9d0ae",
  "crm:c-6": "01a1ef8e-5a8d-76d4-acb6-2bb65a02262a",
  "crm:c-8": "01a1ef8e-5f06-7397-9a0c-445aaacfd3fe",
};
