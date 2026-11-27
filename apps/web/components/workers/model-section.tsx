import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

import type { WorkerFormApi } from "./worker-form";

/** The secret a harness-managed provider authenticates with. */
export function AuthSecretField({ form }: { form: WorkerFormApi }) {
  return (
    <Field
      label="Harness auth secret"
      htmlFor="wp-auth"
      optional
      hint="Only for providers signed in through the harness. Leave blank for ORBIT-managed providers; their credentials are injected at runtime."
    >
      <Input
        id="wp-auth"
        className="font-mono text-base md:text-[13px]"
        value={form.values.authSecret}
        onChange={(event) => form.set({ authSecret: event.target.value })}
        placeholder="orbit-opencode-auth"
      />
    </Field>
  );
}
