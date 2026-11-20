import { NextResponse, type NextRequest } from "next/server";

/**
 * Per-request nonce plus baseline security headers for the web app.
 *
 * The CSP is set on both the request (so Next attaches the nonce to its own inline
 * scripts) and the response (so the browser enforces it). Do not add a CSP in
 * next.config.ts as well: duplicate policies intersect and break the page.
 */
function apiConnectSource(): string {
  const configured = process.env.NEXT_PUBLIC_ORBIT_API_URL?.trim();
  if (!configured) return "'self'";
  try {
    const origin = new URL(configured).origin;
    return origin === "null" ? "'self'" : `'self' ${origin}`;
  } catch {
    return "'self'";
  }
}

function contentSecurityPolicy(nonce: string): string {
  const development = process.env.NODE_ENV !== "production";
  const connectSources = [apiConnectSource(), development ? "ws: wss:" : null]
    .filter(Boolean)
    .join(" ");

  return [
    "default-src 'self'",
    "base-uri 'self'",
    "object-src 'none'",
    "frame-ancestors 'none'",
    "form-action 'self'",
    `script-src 'self' 'nonce-${nonce}'${development ? " 'unsafe-eval'" : ""}`,
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' blob: data:",
    "font-src 'self' data:",
    `connect-src ${connectSources}`,
    "worker-src 'self'",
  ].join("; ");
}

export function proxy(request: NextRequest) {
  const nonce = btoa(crypto.randomUUID());
  const csp = contentSecurityPolicy(nonce);

  const requestHeaders = new Headers(request.headers);
  requestHeaders.set("x-nonce", nonce);
  requestHeaders.set("content-security-policy", csp);

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set("content-security-policy", csp);
  return response;
}

export const config = {
  // Static assets keep their long-lived caching and need no CSP.
  matcher: ["/((?!_next/static|_next/image|favicon.ico|icon.svg|manifest.webmanifest|sw.js).*)"],
};
