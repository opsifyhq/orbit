import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { ApiError } from "./http.ts";
import { streamExecutionEvents, type ParsedSseEvent } from "./sse.ts";

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

const encoder = new TextEncoder();

/** A fetch whose body sends `chunks`, then either closes or stays open forever. */
function fakeStream(chunks: string[], thenHang: boolean) {
  globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        if (!thenHang) controller.close();
        init?.signal?.addEventListener("abort", () =>
          controller.error(new DOMException("aborted", "AbortError")),
        );
      },
    });
    return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
  }) as typeof fetch;
}

test("events are parsed across chunk boundaries and comments are ignored", async () => {
  fakeStream(
    [": keep-alive\n\n", "event: execution.event\nid: 7\nda", 'ta: {"seq":7}\n\n', "event: done\ndata: {}\n\n"],
    false,
  );
  const events: ParsedSseEvent[] = [];
  let opened = false;
  await streamExecutionEvents("exec-1", {
    onOpen: () => (opened = true),
    onEvent: (event) => events.push(event),
  });
  assert.equal(opened, true);
  assert.deepEqual(events, [
    { event: "execution.event", id: "7", data: '{"seq":7}' },
    { event: "done", id: undefined, data: "{}" },
  ]);
});

test("a stream that goes silent is torn down and reported as stalled", async () => {
  fakeStream(["event: snapshot\ndata: {}\n\n"], true);
  const errors: unknown[] = [];
  await streamExecutionEvents("exec-1", {
    idleTimeoutMs: 40,
    onEvent: () => undefined,
    onError: (error) => errors.push(error),
  });
  assert.equal(errors.length, 1);
  assert.ok(errors[0] instanceof ApiError);
  assert.equal((errors[0] as ApiError).code, "stream_stalled");
  // Status 0 is "never reached the server": the hook treats it as retryable.
  assert.equal((errors[0] as ApiError).status, 0);
});

test("the caller's abort ends the stream quietly", async () => {
  fakeStream(["event: snapshot\ndata: {}\n\n"], true);
  const controller = new AbortController();
  const errors: unknown[] = [];
  const finished = streamExecutionEvents("exec-1", {
    signal: controller.signal,
    idleTimeoutMs: 5_000,
    onEvent: () => undefined,
    onError: (error) => errors.push(error),
  });
  setTimeout(() => controller.abort(), 20);
  await finished;
  assert.deepEqual(errors, []);
});
