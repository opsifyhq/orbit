import { Callout } from "@/components/ui/callout";
import { CheckboxField } from "@/components/ui/checkbox";
import { Field } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";

import { FormGroup } from "./shared";
import { nativeToolsError, type WorkerFormApi } from "./worker-form";

/** Web search and fetch for OpenCode. */
export function WebToolFields({ form }: { form: WorkerFormApi }) {
  const { values, set } = form;
  return (
    <div className="grid gap-2 sm:grid-cols-2">
      <CheckboxField
        title="Allow web search"
        description="Non-interactive. Needs a search provider."
        checked={values.allowWebSearch}
        onChange={(event) => set({ allowWebSearch: event.target.checked })}
      />
      <CheckboxField
        title="Allow web fetch"
        description="Fetch known URLs."
        checked={values.allowWebFetch}
        onChange={(event) => set({ allowWebFetch: event.target.checked })}
      />
    </div>
  );
}

/** glab and gh inside the worker workspace. */
export function CliToolFields({ form }: { form: WorkerFormApi }) {
  const { values, set } = form;
  return (
    <>
      <div className="grid gap-2 sm:grid-cols-2">
        <CheckboxField
          title="Allow glab"
          description="Uses GITLAB_TOKEN from a team GitLab connection."
          checked={values.allowGlab}
          onChange={(event) => set({ allowGlab: event.target.checked })}
        />
        <CheckboxField
          title="Allow gh"
          description="Uses GH_TOKEN from a team GitHub connection."
          checked={values.allowGh}
          onChange={(event) => set({ allowGh: event.target.checked })}
        />
      </div>
      {values.allowGlab || values.allowGh ? (
        <Callout tone="warning" title="The token is exposed to shell tools">
          Anything that runs in the worker workspace can read it.
        </Callout>
      ) : null}
    </>
  );
}

/** The JSON map of native tool permissions. */
export function NativeToolFields({ form }: { form: WorkerFormApi }) {
  const error = nativeToolsError(form.values.nativeToolsText);
  return (
    <Field
      label="Permissions for OpenCode"
      htmlFor="wp-native-tools"
      error={error}
      hint="JSON map of tool name to allow, ask or deny. For example, shell allow and bash deny."
    >
      <Textarea
        id="wp-native-tools"
        className="min-h-24 font-mono text-base md:text-xs"
        value={form.values.nativeToolsText}
        onChange={(event) => form.set({ nativeToolsText: event.target.value })}
        aria-invalid={error ? true : undefined}
        placeholder={'{\n  "bash": "deny",\n  "edit": "allow",\n  "shell": "allow"\n}'}
      />
    </Field>
  );
}

export const WEB_TOOLS_GROUP = {
  title: "OpenCode web tools",
  description: "Off by default so headless OpenCode runs do not stop on an interactive web search request.",
};
export const CLI_TOOLS_GROUP = {
  title: "Command-line tools",
  description: "The image must contain the tool you allow.",
};

/** All three permission groups, stacked. */
export function PermissionGroups({ form }: { form: WorkerFormApi }) {
  return (
    <>
      <FormGroup {...WEB_TOOLS_GROUP}>
        <WebToolFields form={form} />
      </FormGroup>
      <FormGroup {...CLI_TOOLS_GROUP}>
        <CliToolFields form={form} />
      </FormGroup>
      <FormGroup title="Native tool permissions">
        <NativeToolFields form={form} />
      </FormGroup>
    </>
  );
}
