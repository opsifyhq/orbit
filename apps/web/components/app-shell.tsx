"use client";

import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";

import { CloudOff, Loader2, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { isNoAccessError, isSessionError } from "@/lib/http";
import { errorMessage } from "@orbit/api-client";
import { rememberNoAccessReason } from "@/lib/no-access";
import { BrandMark, Wordmark } from "@/components/shell/brand";
import { CommandPaletteProvider } from "@/components/shell/command-palette";
import { MobileHeader, MobileTabBar } from "@/components/shell/mobile-nav";
import { pageLabel } from "@/components/shell/nav";
import { Sidebar } from "@/components/shell/sidebar";
import { getAccessToken, setAccessToken } from "@/lib/api";
import { useDashboard, useMe, useMeta } from "@/lib/queries";
import { currentLocationPath, loginPathFor } from "@/lib/safe-redirect";

function isPublicPath(pathname: string) {
  return (
    pathname.startsWith("/login") ||
    pathname.startsWith("/auth/") ||
    pathname.startsWith("/no-access")
  );
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "/";
  const router = useRouter();
  const meta = useMeta();
  const publicPath = isPublicPath(pathname);

  // Public pages (login/callback) have no token yet. Querying /me there caches a 401,
  // which the shell then reuses after the callback redirect and clears the fresh token.
  const me = useMe(!publicPath);
  useEffect(() => {
    // A 403 means the identity is valid but the user has no ORBIT role. Show the
    // dedicated explanation instead of bouncing to login (which loops forever).
    // Check this before the auth-mode guard so it works even without meta (dev mode
    // or a failed /meta call), and only on protected pages so login stays reachable.
    if (!publicPath && isNoAccessError(me.error)) {
      rememberNoAccessReason(errorMessage(me.error));
      router.replace("/no-access");
      return;
    }
    const auth = (meta.data?.auth ?? {}) as { mode?: string };
    if (auth.mode !== "oidc" || publicPath) return;
    const login = loginPathFor(currentLocationPath());
    if (!getAccessToken()) {
      router.replace(login);
    } else if (isSessionError(me.error)) {
      // 401 (expired or foreign token): the session is over. A 403 is handled above;
      // other failures (5xx, network) keep the token and show a retry state below.
      setAccessToken(null);
      router.replace(login);
    }
  }, [meta.data?.auth, me.error, pathname, publicPath, router]);

  const authMode = (meta.data?.auth ?? {}) as { mode?: string };
  const metaUnavailable =
    meta.isError && !isSessionError(meta.error) && !isNoAccessError(meta.error)
      ? {
          message: errorMessage(meta.error),
          retrying: meta.isFetching,
          retry: () => void meta.refetch(),
        }
      : null;
  const sessionUnavailable =
    authMode.mode === "oidc" &&
    !publicPath &&
    !me.data &&
    me.isError &&
    !isSessionError(me.error) &&
    !isNoAccessError(me.error)
      ? {
          message: errorMessage(me.error),
          retrying: me.isFetching,
          retry: () => void me.refetch(),
        }
      : null;
  // A failed /meta is shown ahead of a failed /me: without meta there is no auth mode
  // to interpret the me result, and retrying meta recovers both.
  const unavailable = metaUnavailable ?? sessionUnavailable;

  const title = publicPath ? "Sign in · ORBIT" : `${pageLabel(pathname)} · ORBIT`;
  const hasToken = Boolean(getAccessToken());

  // Don't render the signed-in chrome while OIDC auth is still being resolved. Without
  // this the navigation flashes for a frame before the effect redirects to the IdP.
  if (!publicPath && (meta.isPending || (authMode.mode === "oidc" && (!hasToken || me.isPending)))) {
    return (
      <div
        data-ink
        className="flex min-h-dvh items-center justify-center bg-background"
        role="status"
        aria-label="Checking sign-in"
      >
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (publicPath) {
    // Signed-out screens get no navigation: nothing behind it is reachable yet.
    return (
      <div data-ink className="flex min-h-dvh flex-col bg-background text-ink-foreground">
        <title>{title}</title>
        <div className="flex h-14 items-center gap-2.5 px-5">
          <BrandMark />
          <Wordmark />
        </div>
        <main id="main" className="flex flex-1 items-center justify-center px-4 pb-20">
          {children}
        </main>
      </div>
    );
  }

  return (
    <>
      {/* React hoists <title> into <head>; the root layout sets none, so this is the only one. */}
      <title>{title}</title>
      <SignedInShell pathname={pathname}>
        {unavailable ? (
          <EmptyState
            icon={<CloudOff />}
            title="Can't reach ORBIT right now"
            description={`You are still signed in. ${unavailable.message}`}
            action={
              <Button onClick={unavailable.retry} loading={unavailable.retrying}>
                <RefreshCw /> Retry
              </Button>
            }
          />
        ) : (
          children
        )}
      </SignedInShell>
    </>
  );
}

function SignedInShell({ pathname, children }: { pathname: string; children: React.ReactNode }) {
  const dashboard = useDashboard();
  const signals = {
    pendingApprovals: dashboard.data?.summary.pendingApprovals ?? 0,
    runningExecutions: dashboard.data?.summary.runningExecutions ?? 0,
  };

  return (
    <CommandPaletteProvider>
      <a
        href="#main"
        className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[60] focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-sm focus:font-medium focus:text-primary-foreground"
      >
        Skip to content
      </a>
      <div className="min-h-dvh bg-background">
        <Sidebar pathname={pathname} signals={signals} />
        <MobileHeader />
        <div className="lg:pl-[15.5rem]">
          <main
            id="main"
            // Focusable so the skip link moves keyboard focus, not only the scroll position.
            tabIndex={-1}
            className="mx-auto w-full max-w-[1180px] px-4 pb-28 focus:outline-none pt-5 sm:px-6 lg:px-10 lg:pb-14 lg:pt-14"
          >
            {children}
          </main>
        </div>
        <MobileTabBar pathname={pathname} signals={signals} />
      </div>
    </CommandPaletteProvider>
  );
}
