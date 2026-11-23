// What a call the server refused throws.

import type { ErrorCode, Rejection } from "./types.ts";

/** A call the server answered with an error. */
export class SpoolError extends Error {
  /** The HTTP status. */
  readonly status: number;
  readonly code: ErrorCode;
  /** With `rejected`: the operations of the changeset that broke a rule. */
  readonly rejections: Rejection[];

  constructor(status: number, code: ErrorCode, message: string, rejections: Rejection[] = []) {
    super(message);
    this.name = "SpoolError";
    this.status = status;
    this.code = code;
    this.rejections = rejections;
  }
}

/** The error of a response that is not OK, from what the server said or, failing that, its status. */
export async function failure(response: Response): Promise<SpoolError> {
  const text = await response.text();
  try {
    const { error } = JSON.parse(text) as {
      error: { code: ErrorCode; message: string; rejections?: Rejection[] | null };
    };
    return new SpoolError(response.status, error.code, error.message, error.rejections ?? []);
  } catch {
    // Not the server's own error: something between here and it answered.
    const code: ErrorCode =
      response.status === 401 ? "unauthorized" : response.status === 404 ? "not_found" : "internal";
    return new SpoolError(response.status, code, text.trim() || `the server answered ${response.status}`);
  }
}
