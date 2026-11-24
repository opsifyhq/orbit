import Link from "next/link";
import { Compass } from "lucide-react";

import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";

/** Unknown paths: say so in ORBIT's own words and offer a way back. */
export default function NotFound() {
  return (
    <div className="mx-auto flex min-h-[60vh] max-w-lg items-center">
      <EmptyState
        className="w-full"
        icon={<Compass />}
        title="This page does not exist"
        description="The link may be old, or the item was deleted. Use the search (Ctrl+K or ⌘K) to jump to any page or agent."
        action={
          <div className="flex flex-wrap justify-center gap-2">
            <Button asChild>
              <Link href="/">Go to Overview</Link>
            </Button>
            <Button asChild variant="outline">
              <Link href="/executions">Executions</Link>
            </Button>
          </div>
        }
      />
    </div>
  );
}
