/**
 * Error thrown for any failed API call (HTTP error, unreadable body or network failure).
 *
 * `status` is the HTTP status, or `0` when the request never reached the server. Callers
 * that need to distinguish "your session is invalid" (401), "you may not use ORBIT" (403)
 * from "the API is having a moment" (5xx, network) must look at `status`, never at the message.
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly details?: Array<{ path: string; message: string }>;
  readonly requestId?: string;

  constructor(
    status: number,
    message: string,
    extra: { code?: string; details?: ApiError["details"]; requestId?: string } = {},
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = extra.code;
    this.details = extra.details;
    this.requestId = extra.requestId;
  }
}

/** The request never reached the server (offline, DNS, proxy down). */
export function networkError(): ApiError {
  return new ApiError(0, "Could not reach the ORBIT API. Check your connection and try again.", {
    code: "network_error",
  });
}
