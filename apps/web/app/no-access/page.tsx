"use client";

import { useEffect, useState } from "react";
import { ShieldX } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { readNoAccessReason } from "@/lib/no-access";
import { signOut } from "@/lib/session";

export default function NoAccessPage() {
  const [reason, setReason] = useState<string | null>(null);
  useEffect(() => {
    // The reason is one-shot; StrictMode's second effect run finds nothing, so keep
    // the value from the first run instead of clearing the displayed message.
    const stored = readNoAccessReason();
    if (stored) setReason(stored);
  }, []);

  return (
    <div className="mx-auto flex min-h-[70vh] max-w-lg items-center">
      <Card className="w-full">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <ShieldX className="h-5 w-5 text-destructive" /> No access to ORBIT
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-sm text-muted-foreground">
            Your identity provider account is valid, but your groups are not mapped to an
            ORBIT role or access team. Ask an ORBIT administrator to grant access.
          </p>
          {reason ? (
            <p
              role="status"
              className="break-words rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-[13px] text-ink-foreground"
            >
              {reason}
            </p>
          ) : null}
          <Button onClick={signOut} className="w-full" variant="outline">
            Sign out / switch account
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}
