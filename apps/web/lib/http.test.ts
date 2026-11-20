import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ApiError,
  apiErrorFromBody,
  errorStatus,
  isNoAccessError,
  isPermanentClientError,
  isSessionError,
} from "./http.ts";

test("401 is a session error, 403 is a no-access error", () => {
  assert.equal(isSessionError(new ApiError(401, "unauthorized")), true);
  assert.equal(isSessionError(new ApiError(403, "forbidden")), false);
  assert.equal(isNoAccessError(new ApiError(403, "forbidden")), true);
  assert.equal(isNoAccessError(new ApiError(401, "unauthorized")), false);

  assert.equal(isSessionError(new Error("boom")), false);
  assert.equal(isNoAccessError(new Error("boom")), false);
  assert.equal(errorStatus(new ApiError(500, "boom")), 500);
  assert.equal(errorStatus(new Error("boom")), undefined);
});

test("permanent client errors exclude retryable statuses and auth failures", () => {
  assert.equal(isPermanentClientError(new ApiError(403, "forbidden")), true);
  assert.equal(isPermanentClientError(new ApiError(404, "missing")), true);
  assert.equal(isPermanentClientError(new ApiError(408, "timeout")), false);
  assert.equal(isPermanentClientError(new ApiError(429, "slow down")), false);
  assert.equal(isPermanentClientError(new ApiError(500, "boom")), false);
  assert.equal(isPermanentClientError(new Error("boom")), false);
});

test("403 envelope messages survive as the no-access reason", () => {
  const error = apiErrorFromBody(403, {
    error: { code: "forbidden", message: "No ORBIT role mapped" },
  });
  assert.equal(error.status, 403);
  assert.equal(error.message, "No ORBIT role mapped");
  assert.equal(isNoAccessError(error), true);
  assert.equal(isSessionError(error), false);
});
