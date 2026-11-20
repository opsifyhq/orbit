"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { KeyRound } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { getAccessToken, setAccessToken } from "@/lib/api";
import { isNoAccessError, isSessionError } from "@/lib/http";
import { rememberNoAccessReason } from "@/lib/no-access";
import { beginOidcLogin } from "@/lib/oidc";
import { safeNextPath } from "@/lib/safe-redirect";
import { useMe, useMeta } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

/** Read `?next=` on the client; SSR has no window, so fall back to "/". */
function requestedNextPath(): string {
  if (typeof window === "undefined") return "/";
  return safeNextPath(new URLSearchParams(window.location.search).get("next"));
}

export default function LoginPage() {
  const meta = useMeta();
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const auth = (meta.data?.auth ?? {}) as { mode?: string; issuer?: string };
  const oidc = auth.mode === "oidc";

  const me = useMe();
  useEffect(() => {
    if (!getAccessToken()) return;
    if (me.isSuccess) {
      router.replace(requestedNextPath());
    } else if (isNoAccessError(me.error)) {
      // A 403 is a valid identity with no ORBIT role, not a bad token. Send it to the
      // explanation page instead of silently clearing and looping back here.
      rememberNoAccessReason(errorMessage(me.error));
      router.replace("/no-access");
    } else if (isSessionError(me.error)) {
      // 401 only: expired or foreign token. Clear it instead of treating it as a
      // session. Transient errors (5xx, network) keep it so a blip does not force a
      // new sign-in.
      setAccessToken(null);
    }
  }, [me.isSuccess, me.error, router]);

  function startSignIn() {
    setError(null);
    setStarting(true);
    void beginOidcLogin(requestedNextPath()).catch(
      (reason) => {
        setStarting(false);
        setError(reason instanceof Error ? reason.message : String(reason));
      },
    );
  }

  return (
    <div className="w-full max-w-sm rounded-xl border border-ink-border bg-ink p-6 shadow-overlay sm:p-8">
      <h1 className="font-display text-2xl leading-tight text-ink-foreground">Sign in to ORBIT</h1>

      {meta.isLoading ? (
        <div className="mt-5 space-y-3" aria-busy="true">
          <Skeleton className="h-4 w-3/4 bg-ink-border" />
          <Skeleton className="h-9 w-full bg-ink-border" />
        </div>
      ) : oidc ? (
        <div className="mt-2 space-y-5">
          <p className="text-sm text-ink-muted">
            Continue with your organization&apos;s identity provider.
          </p>
          <Button className="w-full" onClick={startSignIn} loading={starting}>
            <KeyRound /> Continue with single sign-on
          </Button>
          {auth.issuer ? (
            <p className="break-all text-center text-xs text-ink-muted">
              Provider: <span className="font-mono">{auth.issuer}</span>
            </p>
          ) : null}
        </div>
      ) : (
        <div className="mt-2 space-y-5">
          <p className="text-sm text-ink-muted">
            This installation runs in development mode, so there is no password or single
            sign-on. You will be signed in as the built-in development user.
          </p>
          <Button asChild className="w-full">
            <Link href="/">Continue to ORBIT</Link>
          </Button>
        </div>
      )}

      {error ? (
        <p role="alert" className="mt-4 break-words rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-[13px] text-ink-foreground">
          Sign-in could not start: {error}
        </p>
      ) : null}
    </div>
  );
}
