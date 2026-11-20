export const dynamic = 'force-dynamic';

import type { Metadata, Viewport } from "next";
import { headers } from "next/headers";
import "@fontsource-variable/archivo/wdth.css";
import "@fontsource-variable/jetbrains-mono/index.css";
import "./globals.css";
import { Providers } from "@/components/providers";
import { AppShell } from "@/components/app-shell";

export const metadata: Metadata = {
  description:
    "Self-hosted control plane for creating, connecting, running and governing autonomous AI agents.",
  applicationName: "ORBIT",
  manifest: "/manifest.webmanifest",
  appleWebApp: {
    capable: true,
    title: "ORBIT",
    statusBarStyle: "default",
  },
  icons: {
    icon: "/icon.svg",
    apple: "/icon.svg",
  },
  robots: { index: false, follow: false },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  maximumScale: 5,
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#0f0f1a" },
    { media: "(prefers-color-scheme: dark)", color: "#08080e" },
  ],
};

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  const themeScript = `(function(){try{var s=localStorage.getItem("orbit.theme");var d=s?s==="dark":window.matchMedia("(prefers-color-scheme: dark)").matches;document.documentElement.classList.toggle("dark",d);}catch(e){}})();`
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script nonce={nonce} dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body>
        <Providers>
          <AppShell>{children}</AppShell>
        </Providers>
      </body>
    </html>
  );
}
