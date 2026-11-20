"use client";

import { useEffect, useRef, useState } from "react";
import { errorStatus, isPermanentClientError } from "@/lib/http";
import { streamExecutionEvents } from "@/lib/sse";
import type { ExecutionEventRecord, ExecutionRecord } from "@/lib/types";
import { isTerminalStatus } from "@/lib/utils";

const BACKOFF_BASE_MS = 1_000;
const BACKOFF_CAP_MS = 30_000;

/** Exponential backoff with jitter (50-100 % of the step) so tabs do not reconnect in lockstep. */
function reconnectDelay(attempt: number) {
  const step = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** attempt);
  return step * (0.5 + Math.random() / 2);
}

/** The server refused the stream for good (401/403/404...): retrying cannot help. */
export interface StreamFatalError {
  status: number;
  message: string;
}

/**
 * Live execution subscription. Merges replayed SSE events with any already-fetched
 * events (deduplicated by `seq`) and keeps the execution snapshot fresh.
 *
 * Reconnects with exponential backoff on 5xx, network errors and unexpected stream
 * ends. Permanent 4xx responses stop the loop and surface as `fatal` (401 additionally
 * goes through the shared sign-out path in `lib/http.ts`).
 */
export function useExecutionStream(
  executionId: string | undefined,
  options: {
    /** Hold the connection until the first page of events has loaded over REST. */
    enabled?: boolean;
    /** Highest `seq` already loaded; the stream replays only what comes after it. */
    afterSeq?: number;
  } = {},
) {
  const enabled = options.enabled ?? true;
  const startAfterSeq = useRef(0);
  startAfterSeq.current = options.afterSeq ?? 0;
  const [execution, setExecution] = useState<ExecutionRecord | null>(null);
  const [events, setEvents] = useState<ExecutionEventRecord[]>([]);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fatal, setFatal] = useState<StreamFatalError | null>(null);
  const [done, setDone] = useState(false);
  const lastSeq = useRef(0);

  useEffect(() => {
    if (!executionId || !enabled) return;
    // A different execution (same mounted page) must not show the previous one's data.
    lastSeq.current = startAfterSeq.current;
    setExecution(null);
    setEvents([]);
    setConnected(false);
    setError(null);
    setFatal(null);
    setDone(false);

    const controller = new AbortController();
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;
    let attempt = 0;

    // Events arrive in bursts (a replay can be thousands). Collect them and merge once
    // per frame: one render and one linear merge per burst instead of one per event.
    let pending: ExecutionEventRecord[] = [];
    let flushTimer: ReturnType<typeof setTimeout> | undefined;
    const flush = () => {
      flushTimer = undefined;
      const batch = pending;
      pending = [];
      if (!batch.length) return;
      setEvents((current) => {
        const last = current.length ? current[current.length - 1].seq : -Infinity;
        // The common case: everything new is newer than everything shown.
        if (batch[0].seq > last && batch.every((event, i) => i === 0 || event.seq > batch[i - 1].seq)) {
          return [...current, ...batch];
        }
        const bySeq = new Map<number, ExecutionEventRecord>();
        for (const event of current) bySeq.set(event.seq, event);
        for (const event of batch) bySeq.set(event.seq, event);
        return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
      });
    };

    async function connect() {
      let failure: unknown;
      let progressed = false;
      let sawTerminalSnapshot = false;

      await streamExecutionEvents(executionId!, {
        signal: controller.signal,
        afterSeq: lastSeq.current,
        onOpen: () => {
          setConnected(true);
          setError(null);
        },
        onEvent: (event) => {
          // Only data proves the connection is healthy; a stream that opens and
          // closes immediately keeps backing off.
          if (!progressed) {
            progressed = true;
            attempt = 0;
          }
          if (event.event === "snapshot") {
            try {
              const snapshot = JSON.parse(event.data) as ExecutionRecord;
              sawTerminalSnapshot = isTerminalStatus(snapshot.status);
              setExecution(snapshot);
            } catch {
              // ignore malformed snapshot
            }
            return;
          }
          if (event.event === "execution.event") {
            try {
              const parsed = JSON.parse(event.data) as ExecutionEventRecord;
              if (typeof event.id === "string") {
                lastSeq.current = Math.max(lastSeq.current, Number(event.id) || 0);
              }
              lastSeq.current = Math.max(lastSeq.current, parsed.seq);
              pending.push(parsed);
              if (!flushTimer) flushTimer = setTimeout(flush, 50);
            } catch {
              // ignore malformed event
            }
            return;
          }
          if (event.event === "done") {
            try {
              setExecution(JSON.parse(event.data) as ExecutionRecord);
            } catch {
              // ignore
            }
            flush();
            setDone(true);
            setConnected(false);
            stopped = true;
            return;
          }
          if (event.event === "error") {
            setError(event.data);
          }
        },
        onError: (streamError) => {
          failure = streamError;
        },
      });

      setConnected(false);
      if (stopped || controller.signal.aborted) return;

      if (failure !== undefined) {
        if (isPermanentClientError(failure)) {
          setError(null);
          setFatal({
            status: errorStatus(failure) ?? 0,
            message: failure instanceof Error ? failure.message : String(failure),
          });
          return;
        }
        setError(failure instanceof Error ? failure.message : String(failure));
      } else if (sawTerminalSnapshot) {
        // The run already finished and the server closed the stream; nothing more will come.
        return;
      }
      // A clean end without `done` is a dropped connection: reconnect, but with backoff.

      reconnectTimer = setTimeout(() => {
        reconnectTimer = undefined;
        void connect();
      }, reconnectDelay(attempt));
      attempt += 1;
    }

    // Coming back to the tab: do not sit out the rest of a backoff delay.
    const onVisible = () => {
      if (document.visibilityState !== "visible" || stopped || !reconnectTimer) return;
      clearTimeout(reconnectTimer);
      reconnectTimer = undefined;
      attempt = 0;
      void connect();
    };
    document.addEventListener("visibilitychange", onVisible);

    void connect();
    return () => {
      stopped = true;
      controller.abort();
      document.removeEventListener("visibilitychange", onVisible);
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (flushTimer) clearTimeout(flushTimer);
    };
  }, [executionId, enabled]);

  return { execution, events, connected, done, error, fatal };
}
