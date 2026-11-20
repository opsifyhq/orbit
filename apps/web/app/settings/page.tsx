"use client";

import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, KeyRound, LogOut, Plus } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Disclosure } from "@/components/ui/disclosure";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { api } from "@/lib/api";
import { request } from "@/lib/http";
import { signOut } from "@/lib/session";
import { copyText } from "@/lib/browser";
import {
  queryKeys,
  unwrap,
  useApiTokens,
  useAuditLogs,
  useLicense,
  useMe,
  useMeta,
  useHarnesses,
} from "@/lib/queries";
import { errorMessage, type components } from "@orbit/api-client";
import { formatDate, formatDateTime, relativeTime, titleCase } from "@/lib/utils";
import { humanizeIdentifier } from "@/lib/presentation";

/* ----------------------------------------------------------------------------
 * Shared helpers
 * ------------------------------------------------------------------------- */

const PERMISSION_LABELS: Record<string, string> = {
  "agents:read": "View agents",
  "agents:write": "Create and edit agents",
  "executions:read": "View executions",
  "executions:create": "Start executions",
  "executions:cancel": "Cancel executions",
  "approvals:decide": "Decide approvals",
  "harnesses:read": "View harnesses",
  "providers:read": "View model providers",
  "providers:write": "Manage model providers",
  "connections:read": "View connections",
  "connections:write": "Manage connections",
  "triggers:write": "Manage triggers",
  "settings:read": "View settings",
  "settings:write": "Change settings",
  "members:read": "View members",
  "audit:read": "View audit log",
};

const permissionLabel = (permission: string) => PERMISSION_LABELS[permission] ?? permission;

/** Labels for server identifiers ("apiTokens" → "API tokens", "custom_roles" → "Custom roles"). */
const humanize = (value: string) => humanizeIdentifier(value);

const TABS = [
  { id: "account", label: "Account" },
  { id: "tokens", label: "API tokens" },
  { id: "sso", label: "Single sign-on" },
  { id: "members", label: "Members and roles" },
  { id: "platform", label: "Platform" },
  { id: "audit", label: "Audit log" },
] as const;
type TabId = (typeof TABS)[number]["id"];

function ListSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <Card className="overflow-hidden">
      {Array.from({ length: rows }).map((_, index) => (
        <div key={index} className="border-b px-5 py-3.5 last:border-b-0">
          <Skeleton className="h-9" />
        </div>
      ))}
    </Card>
  );
}

/* ----------------------------------------------------------------------------
 * Page
 * ------------------------------------------------------------------------- */

export default function SettingsPage() {
  const me = useMe();
  const canManage = me.data?.permissions.includes("settings:write") ?? false;
  const [tab, setTab] = useState<TabId>("account");

  // The selected tab lives in the URL hash (#tokens) so a tab can be linked to.
  useEffect(() => {
    const read = () => {
      const hash = window.location.hash.slice(1);
      if (TABS.some((item) => item.id === hash)) setTab(hash as TabId);
    };
    read();
    window.addEventListener("hashchange", read);
    return () => window.removeEventListener("hashchange", read);
  }, []);

  function selectTab(next: string) {
    setTab(next as TabId);
    try {
      window.history.replaceState(null, "", `#${next}`);
    } catch {
      // The tab still switches; only the link is lost.
    }
  }

  const visibleTabs = TABS.filter((item) => item.id !== "members" || canManage);
  const activeTab = visibleTabs.some((item) => item.id === tab) ? tab : "account";

  return (
    <div className="space-y-6">
      <PageHeader
        title="Settings"
        description="Your account, API access, sign-in, who can do what, and how this installation is set up."
        actions={
          <Button variant="outline" onClick={() => signOut()}>
            <LogOut /> Sign out
          </Button>
        }
      />

      <Tabs value={activeTab} onValueChange={selectTab}>
        <TabsList aria-label="Settings sections">
          {visibleTabs.map((item) => (
            <TabsTrigger key={item.id} value={item.id}>
              {item.label}
            </TabsTrigger>
          ))}
        </TabsList>

        <TabsContent value="account">
          <AccountTab />
        </TabsContent>
        <TabsContent value="tokens">
          <TokensTab />
        </TabsContent>
        <TabsContent value="sso">
          <SsoTab />
        </TabsContent>
        {canManage ? (
          <TabsContent value="members">
            <MembersTab />
          </TabsContent>
        ) : null}
        <TabsContent value="platform">
          <PlatformTab />
        </TabsContent>
        <TabsContent value="audit">
          <AuditTab />
        </TabsContent>
      </Tabs>
    </div>
  );
}

/* ----------------------------------------------------------------------------
 * Account
 * ------------------------------------------------------------------------- */

function AccountTab() {
  const me = useMe();

  if (me.isLoading) return <Skeleton className="h-64 rounded-xl" />;
  if (!me.data) {
    return (
      <Callout tone="danger" title="Could not load your account">
        {errorMessage(me.error)}
      </Callout>
    );
  }
  const { user, organization, permissions, authMethod } = me.data;

  return (
    <div className="max-w-3xl space-y-5">
      <div className="grid gap-5 sm:grid-cols-2">
        <Card className="p-4">
          <h2 className="mb-3 text-[15px] font-semibold">You</h2>
          <DescriptionList
            items={[
              { label: "Name", value: user.displayName },
              { label: "Email", value: user.email },
              { label: "Role", value: <Badge variant="default">{titleCase(user.role)}</Badge> },
              { label: "Signed in with", value: titleCase(authMethod) },
            ]}
          />
        </Card>
        <Card className="p-4">
          <h2 className="mb-3 text-[15px] font-semibold">Organization</h2>
          <DescriptionList
            items={[
              { label: "Name", value: organization.name },
              { label: "Slug", value: organization.slug, mono: true },
              { label: "Mode", value: titleCase(organization.mode) },
            ]}
          />
        </Card>
      </div>
        <Disclosure
          title={`What ${titleCase(user.role)} can do`}
          summary={`${permissions.length} permission${permissions.length === 1 ? "" : "s"}`}
        >
          {permissions.length ? (
            <ul className="grid gap-x-6 gap-y-1.5 text-[13px] sm:grid-cols-2" aria-label="Your permissions">
              {permissions.map((permission) => (
                <li key={permission} className="flex items-baseline justify-between gap-3">
                  <span>{permissionLabel(permission)}</span>
                  <span className="font-mono text-xs text-muted-foreground">{permission}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">Your role has no permissions.</p>
          )}
        </Disclosure>
    </div>
  );
}

/* ----------------------------------------------------------------------------
 * API tokens
 * ------------------------------------------------------------------------- */

function TokensTab() {
  const tokens = useApiTokens();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [tokenName, setTokenName] = useState("automation");
  // The value is shown once and cannot be fetched again, so it must survive anything
  // that does not concern this very token.
  const [createdToken, setCreatedToken] = useState<{ id: string; value: string } | null>(null);
  const [copied, setCopied] = useState(false);

  const createToken = useMutation({
    mutationFn: async () =>
      unwrap(
        await api.POST("/api/v1/api-tokens", {
          body: { name: tokenName.trim() || "automation" },
        }),
      ),
    onSuccess: (result) => {
      setCreatedToken({ id: result.id, value: result.token });
      setCopied(false);
      setDialogOpen(false);
      setTokenName("automation");
      void queryClient.invalidateQueries({ queryKey: queryKeys.tokens });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const revokeToken = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/api-tokens/{token_id}", {
          params: { path: { token_id: id } },
        }),
      ),
    onSuccess: (_, id) => {
      toast.success("Token revoked");
      // Hide the one-time value only when it belongs to the token just revoked.
      setCreatedToken((current) => (current?.id === id ? null : current));
      void queryClient.invalidateQueries({ queryKey: queryKeys.tokens });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function copyToken() {
    if (!createdToken) return;
    const ok = await copyText(createdToken.value);
    if (ok) {
      setCopied(true);
      toast.success("Token copied");
    } else {
      toast.error("Copying is not available here. Select the token and copy it by hand.");
    }
  }

  async function revoke(id: string, name: string) {
    const ok = await confirm({
      title: `Revoke token ${name}?`,
      description: "Anything that uses it stops working right away. This cannot be undone.",
      confirmLabel: "Revoke token",
      destructive: true,
    });
    if (ok) revokeToken.mutate(id);
  }

  return (
    <div className="space-y-6">
      {createdToken ? (
        <Callout
          tone="warning"
          title="Copy your new token now"
          icon={<KeyRound />}
          action={
            <Button variant="outline" onClick={() => setCreatedToken(null)}>
              I have saved it
            </Button>
          }
        >
          <div className="space-y-3 pt-1">
            <p>You will not see this token again. ORBIT keeps only a hash of it.</p>
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
              <code
                className="block min-w-0 flex-1 select-all break-all rounded-md border bg-card px-3 py-2 font-mono text-xs text-foreground"
              >
                {createdToken.value}
              </code>
              <Button onClick={() => void copyToken()} className="shrink-0">
                {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy token"}
              </Button>
            </div>
          </div>
        </Callout>
      ) : null}

      <Section
        title="API tokens"
        count={tokens.data?.length}
        description="Tokens let CI, scripts and other clients call the ORBIT API. Send one as a bearer token in the Authorization header."
        actions={
          <Button onClick={() => setDialogOpen(true)}>
            <Plus /> New token
          </Button>
        }
      >
        {tokens.isLoading ? (
          <ListSkeleton rows={2} />
        ) : tokens.isError ? (
          <Callout tone="danger" title="Could not load tokens">
            {errorMessage(tokens.error)}
          </Callout>
        ) : tokens.data?.length ? (
          <Card className="overflow-hidden">
            {tokens.data.map((token) => (
              <div
                key={token.id}
                className="flex flex-col gap-2 border-b px-4 py-3.5 last:border-b-0 sm:flex-row sm:items-center sm:justify-between sm:px-5"
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="truncate text-sm font-medium">{token.name}</p>
                    {token.revokedAt ? <Badge variant="secondary">Revoked</Badge> : null}
                  </div>
                  <p className="mt-0.5 flex flex-wrap items-baseline gap-x-4 gap-y-0.5 text-[13px] text-muted-foreground">
                    <span className="font-mono text-xs">{token.tokenPrefix}…</span>
                    <span>
                      {token.scopes.length
                        ? `${token.scopes.length} ${token.scopes.length === 1 ? "scope" : "scopes"}`
                        : "Inherits your role"}
                    </span>
                    <span>
                      {token.revokedAt
                        ? `Revoked ${relativeTime(token.revokedAt)}`
                        : token.lastUsedAt
                          ? `Used ${relativeTime(token.lastUsedAt)}`
                          : "Never used"}
                    </span>
                  </p>
                </div>
                {token.revokedAt ? null : (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="shrink-0 self-start text-destructive hover:text-destructive sm:self-auto"
                    onClick={() => void revoke(token.id, token.name)}
                    loading={revokeToken.isPending && revokeToken.variables === token.id}
                  >
                    Revoke
                  </Button>
                )}
              </div>
            ))}
          </Card>
        ) : (
          <EmptyState
            icon={<KeyRound />}
            title="No API tokens yet"
            description="Create one for CI, scripts or other clients. You see its value once, right after creating it."
            action={
              <Button onClick={() => setDialogOpen(true)}>
                <Plus /> New token
              </Button>
            }
          />
        )}
      </Section>

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New API token</DialogTitle>
            <DialogDescription>
              The token can do whatever your role can do, unless its scopes are restricted.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (!createToken.isPending) createToken.mutate();
            }}
          >
            <Field label="Name" htmlFor="token-name" hint="Say what will use it, for example ci-deploy.">
              <Input
                id="token-name"
                value={tokenName}
                placeholder="ci-deploy"
                autoFocus
                onChange={(event) => setTokenName(event.target.value)}
              />
            </Field>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setDialogOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" loading={createToken.isPending}>
                Create token
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/* ----------------------------------------------------------------------------
 * Single sign-on
 * ------------------------------------------------------------------------- */

function SsoTab() {
  return (
    <div className="space-y-6">
      <Callout tone="info" title="Applied when the API starts">
        These settings are stored by ORBIT and merged over the environment values in OIDC mode. Restart
        the API after saving. Fields you leave blank keep the environment defaults.
      </Callout>
      <Section
        title="OpenID Connect"
        description="Let people sign in with your identity provider, such as Keycloak, Dex or Okta."
      >
        <Card className="p-4 sm:p-5">
          <OidcSettingsForm />
        </Card>
      </Section>
    </div>
  );
}

type OidcSettings = components["schemas"]["OidcSettings"];

const oidcQueryKey = ["settings", "oidc"] as const;

function OidcSettingsForm() {
  const settings = useQuery({
    queryKey: oidcQueryKey,
    queryFn: async () => unwrap(await api.GET("/api/v1/settings/oidc")),
  });

  if (settings.isError) {
    return (
      <Callout
        tone="danger"
        title="Could not load the sign-in settings"
      >
        <p>{errorMessage(settings.error)}</p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="mt-3"
          loading={settings.isFetching}
          onClick={() => void settings.refetch()}
        >
          Try again
        </Button>
      </Callout>
    );
  }
  if (!settings.data) {
    return (
      <div className="space-y-4" aria-busy="true">
        <Skeleton className="h-9" />
        <Skeleton className="h-9" />
        <Skeleton className="h-9" />
      </div>
    );
  }
  return <OidcSettingsEditor loaded={settings.data} />;
}

function OidcSettingsEditor({ loaded }: { loaded: OidcSettings }) {
  const queryClient = useQueryClient();
  const [issuer, setIssuer] = useState(loaded.issuer ?? "");
  const [clientId, setClientId] = useState(loaded.clientId ?? "");
  const [redirectUri, setRedirectUri] = useState(loaded.redirectUri ?? "");
  const [scopes, setScopes] = useState(loaded.scopes || "openid profile email");
  const [audience, setAudience] = useState(loaded.audience ?? "");
  const [jwksUrl, setJwksUrl] = useState(loaded.jwksUrl ?? "");
  const [autoProvision, setAutoProvision] = useState(loaded.autoProvision ?? true);
  const [defaultRole, setDefaultRole] = useState(loaded.defaultRole || "viewer");
  const [emailClaim, setEmailClaim] = useState(loaded.emailClaim || "email");
  const [requireMappedGroup, setRequireMappedGroup] = useState(loaded.requireMappedGroup ?? false);

  const saveSettings = useMutation({
    mutationFn: async (edited: Partial<OidcSettings>) =>
      // Send the whole loaded object with the edits on top, so fields this form does not
      // render (role claim, group/team mappings, sync) are never reset by a save.
      unwrap(await api.PUT("/api/v1/settings/oidc", { body: { settings: { ...loaded, ...edited } } })),
    onSuccess: (saved) => {
      queryClient.setQueryData(oidcQueryKey, saved);
      toast.success("Sign-in settings saved. Restart the API to apply them.");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  const saving = saveSettings.isPending;

  function save() {
    // These values take effect on the next restart; a typo here locks people out.
    if (issuer.trim() && !/^https?:\/\/\S+$/.test(issuer.trim())) {
      toast.error("The issuer must be a full URL, for example https://accounts.example.com");
      document.getElementById("oidc-issuer")?.focus();
      return;
    }
    saveSettings.mutate({
      issuer,
      clientId,
      redirectUri,
      scopes,
      audience,
      jwksUrl,
      autoProvision,
      defaultRole,
      emailClaim,
      requireMappedGroup,
    });
  }

  return (
    <form
      className="space-y-5"
      onSubmit={(event) => {
        event.preventDefault();
        save();
      }}
    >
      <Field label="Issuer URL" htmlFor="oidc-issuer" hint="The address of your identity provider.">
        <Input
          id="oidc-issuer"
          className="font-mono"
          value={issuer}
          onChange={(e) => setIssuer(e.target.value)}
          placeholder="https://accounts.example.com"
        />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Client ID" htmlFor="oidc-client-id">
          <Input
            id="oidc-client-id"
            className="font-mono"
            value={clientId}
            onChange={(e) => setClientId(e.target.value)}
            placeholder="orbit-web"
          />
        </Field>
        <Field label="Redirect URI" htmlFor="oidc-redirect" hint="Register this exact address with your provider.">
          <Input
            id="oidc-redirect"
            className="font-mono"
            value={redirectUri}
            onChange={(e) => setRedirectUri(e.target.value)}
            placeholder="http://localhost:3000/auth/callback"
          />
        </Field>
        <Field label="Default role" htmlFor="oidc-role" hint="Role given to people who sign in for the first time.">
          <Select id="oidc-role" value={defaultRole} onChange={(e) => setDefaultRole(e.target.value)}>
            {[...new Set([defaultRole, ...BUILTIN_ROLES])].filter(Boolean).map((role) => (
              <option key={role} value={role}>
                {humanize(role)}
              </option>
            ))}
          </Select>
        </Field>
      </div>

      <CheckboxField
        title="Create users on first sign-in"
        description="A local user is created the first time someone signs in through your provider."
        checked={autoProvision}
        onChange={(e) => setAutoProvision(e.target.checked)}
      />

      <Disclosure title="Advanced settings" summary="Scopes, audience, JWKS URL and identity claim">
<div className="grid gap-4 sm:grid-cols-2">
          <Field label="Scopes" htmlFor="oidc-scopes">
            <Input
              id="oidc-scopes"
              className="font-mono"
              value={scopes}
              onChange={(e) => setScopes(e.target.value)}
              placeholder="openid profile email"
            />
          </Field>
          <Field label="Audience" htmlFor="oidc-audience">
            <Input
              id="oidc-audience"
              className="font-mono"
              value={audience}
              onChange={(e) => setAudience(e.target.value)}
              placeholder="orbit-api"
            />
          </Field>
          <Field label="JWKS URL" htmlFor="oidc-jwks" className="sm:col-span-2">
            <Input
              id="oidc-jwks"
              className="font-mono"
              value={jwksUrl}
              onChange={(e) => setJwksUrl(e.target.value)}
              placeholder="https://accounts.example.com/.well-known/jwks.json"
            />
          </Field>
          <Field
            label="Identity claim"
            htmlFor="oidc-claim"
            hint="Use name when several accounts at your provider share one email address."
            className="sm:col-span-2"
          >
            <Input
              id="oidc-claim"
              className="font-mono"
              value={emailClaim}
              onChange={(e) => setEmailClaim(e.target.value)}
              placeholder="email or name"
            />
          </Field>
        </div>
      </Disclosure>

      <div>
        <Button type="submit" loading={saving}>
          Save sign-in settings
        </Button>
      </div>
    </form>
  );
}

/* ----------------------------------------------------------------------------
 * Members and roles
 * ------------------------------------------------------------------------- */

const PERMISSIONS = Object.keys(PERMISSION_LABELS);

type UserRow = {
  id: string;
  email: string;
  displayName: string;
  role: string;
  status: string;
  oidcSubject?: string | null;
};
type RoleRow = { id: string; name: string; permissions: string[] };
const BUILTIN_ROLES = ["owner", "admin", "operator", "viewer"];

function MembersTab() {
  return (
    <div className="space-y-8">
      <AccessPolicySection />
      <UsersSection />
      <RolesSection />
    </div>
  );
}

function AccessPolicySection() {
  const queryClient = useQueryClient();
  const policy = useQuery({
    queryKey: ["access-settings"],
    queryFn: () => request<{ teamIsolation: boolean }>("/api/v1/settings/access"),
  });
  const save = useMutation({
    mutationFn: (teamIsolation: boolean) =>
      request("/api/v1/settings/access", {
        method: "PUT",
        body: JSON.stringify({ teamIsolation }),
      }),
    onSuccess: () => {
      toast.success("Access policy updated");
      void queryClient.invalidateQueries({ queryKey: ["access-settings"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  return (
    <Section title="Team isolation" description="Applies as soon as you change it.">
      <Card className="flex items-start justify-between gap-4 p-4">
        <div className="min-w-0">
          <p id="team-isolation-label" className="text-sm font-medium">
            Strict team isolation
          </p>
          <p className="mt-0.5 text-[13px] text-muted-foreground">
            People who are not admins see only private resources and resources owned by their access
            teams. Owners and admins still see everything. Existing users are members of the Default
            team.
          </p>
        </div>
        <Switch
          aria-labelledby="team-isolation-label"
          checked={policy.data?.teamIsolation ?? true}
          disabled={policy.isLoading || save.isPending}
          onCheckedChange={(checked) => save.mutate(checked)}
        />
      </Card>
    </Section>
  );
}

function UsersSection() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const me = useMe();
  const currentUserId = (me.data?.user as { id?: string } | undefined)?.id;
  const users = useQuery({
    queryKey: ["users"],
    queryFn: () => request<UserRow[]>("/api/v1/users"),
  });
  const roles = useQuery({
    queryKey: ["custom-roles"],
    queryFn: () => request<RoleRow[]>("/api/v1/roles"),
  });

  const setRole = useMutation({
    mutationFn: ({ id, role }: { id: string; role: string }) =>
      request(`/api/v1/users/${id}`, {
        method: "PATCH",
        body: JSON.stringify({ role }),
      }),
    onSuccess: () => {
      toast.success("Role updated");
      void queryClient.invalidateQueries({ queryKey: ["users"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const roleOptions = [...BUILTIN_ROLES, ...(roles.data ?? []).map((role) => role.name)];

  return (
    <Section
      title="People"
      count={users.data?.length}
      description="Everyone in this organization. A role change applies as soon as you pick it."
    >
      {users.isLoading ? (
        <ListSkeleton rows={3} />
      ) : users.isError ? (
        <Callout tone="danger" title="Could not load people">
          {errorMessage(users.error)}
        </Callout>
      ) : (
        <Card className="overflow-hidden">
          {users.data?.map((user) => (
            <div
              key={user.id}
              className="flex flex-col gap-2 border-b px-4 py-3 last:border-b-0 sm:flex-row sm:items-center sm:justify-between sm:px-5"
            >
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="truncate text-sm font-medium">{user.displayName}</p>
                  {user.status !== "active" ? (
                    <Badge variant="secondary">{titleCase(user.status)}</Badge>
                  ) : null}
                  {user.oidcSubject ? <Badge variant="outline">Single sign-on</Badge> : null}
                </div>
                <p className="truncate text-[13px] text-muted-foreground">{user.email}</p>
              </div>
              <Select
                aria-label={`Role for ${user.displayName}`}
                className="h-8 w-full text-[13px] sm:w-40"
                value={user.role}
                disabled={setRole.isPending && setRole.variables?.id === user.id}
                onChange={async (event) => {
                  const role = event.target.value;
                  // Changing your own role can lock you out of this page.
                  if (user.id === currentUserId) {
                    const confirmed = await confirm({
                      title: `Change your own role to ${humanize(role)}?`,
                      description:
                        "You may lose access to settings straight away, and only another owner can change it back.",
                      confirmLabel: "Change my role",
                      destructive: true,
                    });
                    if (!confirmed) return;
                  }
                  setRole.mutate({ id: user.id, role });
                }}
              >
                {[...new Set([user.role, ...roleOptions])].map((role) => (
                  <option key={role} value={role}>
                    {humanize(role)}
                  </option>
                ))}
              </Select>
            </div>
          ))}
        </Card>
      )}
    </Section>
  );
}

function RolesSection() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const roles = useQuery({
    queryKey: ["custom-roles"],
    queryFn: () => request<RoleRow[]>("/api/v1/roles"),
  });
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<RoleRow | null>(null);
  const [name, setName] = useState("");
  const [permissions, setPermissions] = useState<string[]>([]);

  const saveRole = useMutation({
    mutationFn: () =>
      request(editing ? `/api/v1/roles/${editing.id}` : "/api/v1/roles", {
        method: editing ? "PATCH" : "POST",
        body: JSON.stringify({ name: name.trim(), permissions }),
      }),
    onSuccess: () => {
      toast.success(editing ? "Role updated" : "Role created");
      setOpen(false);
      setEditing(null);
      void queryClient.invalidateQueries({ queryKey: ["custom-roles"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const removeRole = useMutation({
    mutationFn: (id: string) => request(`/api/v1/roles/${id}`, { method: "DELETE" }),
    onSuccess: () => {
      toast.success("Role deleted");
      void queryClient.invalidateQueries({ queryKey: ["custom-roles"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  function openCreate() {
    setEditing(null);
    setName("");
    setPermissions([]);
    setOpen(true);
  }
  function openEdit(role: RoleRow) {
    setEditing(role);
    setName(role.name);
    setPermissions(role.permissions);
    setOpen(true);
  }

  async function deleteRole(role: RoleRow) {
    const ok = await confirm({
      title: `Delete role ${role.name}?`,
      description: "People who have this role lose its permissions. This cannot be undone.",
      confirmLabel: "Delete role",
      destructive: true,
    });
    if (ok) removeRole.mutate(role.id);
  }

  const license = useLicense();
  // Until the license is known, leave the action available; the API still enforces it.
  const rolesLicensed =
    !license.isSuccess || Boolean(license.data?.license.features.includes("custom_roles"));

  const missing = !name.trim() ? "Give the role a name." : !permissions.length ? "Choose at least one permission." : null;

  return (
    <Section
      title="Custom roles"
      count={roles.data?.length}
      description="Add your own roles on top of owner, admin, operator and viewer. Permissions add to the organization role; team access is separate."
      actions={
        <Button
          variant="outline"
          onClick={openCreate}
          disabled={!rolesLicensed}
          title={rolesLicensed ? undefined : "Custom roles are a licensed feature"}
        >
          <Plus /> New role
        </Button>
      }
    >
      {roles.isLoading ? (
        <ListSkeleton rows={1} />
      ) : roles.isError ? (
        <Callout tone="danger" title="Could not load roles">
          {errorMessage(roles.error)}
        </Callout>
      ) : roles.data?.length ? (
        <Card className="overflow-hidden">
          {roles.data.map((role) => (
            <div
              key={role.id}
              className="flex flex-col gap-3 border-b px-4 py-3.5 last:border-b-0 sm:flex-row sm:items-start sm:justify-between sm:px-5"
            >
              <div className="min-w-0 space-y-1.5">
                <p className="text-sm font-medium">{role.name}</p>
                <div className="flex flex-wrap gap-1.5">
                  {role.permissions.map((permission) => (
                    <Badge key={permission} variant="outline" title={permission}>
                      {permissionLabel(permission)}
                    </Badge>
                  ))}
                </div>
              </div>
              <div className="flex shrink-0 gap-1">
                <Button size="sm" variant="outline" onClick={() => openEdit(role)}>
                  Edit
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-destructive hover:text-destructive"
                  onClick={() => void deleteRole(role)}
                >
                  Delete
                </Button>
              </div>
            </div>
          ))}
        </Card>
      ) : (
        <EmptyState
          title="No custom roles"
          description="Custom roles are a licensed feature. Until a license that includes them is installed, use the built-in roles."
        />
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit role ${editing.name}` : "New custom role"}</DialogTitle>
            <DialogDescription>
              Permissions add to the person&apos;s organization role. Team access is separate.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (!missing && !saveRole.isPending) saveRole.mutate();
            }}
          >
            <Field label="Name" htmlFor="role-name">
              <Input id="role-name" value={name} onChange={(event) => setName(event.target.value)} />
            </Field>
            <fieldset className="space-y-2">
              <legend className="text-sm font-medium">Permissions</legend>
              <div className="grid max-h-72 gap-2 overflow-y-auto pr-1 sm:grid-cols-2">
                {PERMISSIONS.map((permission) => (
                  <CheckboxField
                    key={permission}
                    title={permissionLabel(permission)}
                    description={<span className="font-mono text-xs">{permission}</span>}
                    checked={permissions.includes(permission)}
                    onChange={(event) =>
                      setPermissions((current) =>
                        event.target.checked
                          ? [...current, permission]
                          : current.filter((item) => item !== permission),
                      )
                    }
                  />
                ))}
              </div>
            </fieldset>
            <DialogFooter className="items-center">
              {missing ? (
                <p className="text-[13px] text-muted-foreground sm:mr-auto">{missing}</p>
              ) : null}
              <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" loading={saveRole.isPending} disabled={Boolean(missing)}>
                {editing ? "Save role" : "Create role"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

/* ----------------------------------------------------------------------------
 * Platform
 * ------------------------------------------------------------------------- */

function PlatformTab() {
  const meta = useMeta();
  const license = useLicense();
  const authMode = String((meta.data?.auth as { mode?: string } | undefined)?.mode ?? "");

  return (
    <div className="space-y-8">
      <Section title="This installation">
        {meta.isLoading ? (
          <Skeleton className="h-40 rounded-xl" />
        ) : meta.data ? (
          <div className="grid gap-4 lg:grid-cols-2">
            <Card className="p-4">
              <DescriptionList
                items={[
                  { label: "Version", value: meta.data.version },
                  { label: "API version", value: meta.data.apiVersion },
                  { label: "Environment", value: meta.data.environment },
                  {
                    label: "Sign-in mode",
                    value: authMode ? (authMode === "oidc" ? "Single sign-on (OIDC)" : titleCase(authMode)) : "Unknown",
                  },
                  { label: "Event transport", value: meta.data.eventsDriver },
                ]}
              />
            </Card>
            <Card className="p-4">
              <h3 className="mb-3 text-[13px] font-medium text-muted-foreground">Features</h3>
              <ul className="divide-y text-[13px]">
                {Object.entries(meta.data.features).map(([feature, enabled]) => (
                  <li
                    key={feature}
                    className="flex items-center justify-between gap-3 py-2 first:pt-0 last:pb-0"
                  >
                    <span>{humanize(feature)}</span>
                    <Badge variant={enabled ? "success" : "outline"}>
                      {enabled ? "Enabled" : "Not enabled"}
                    </Badge>
                  </li>
                ))}
              </ul>
            </Card>
          </div>
        ) : (
          <Callout tone="danger" title="Could not load platform details">
            {errorMessage(meta.error)}
          </Callout>
        )}
      </Section>

      <DefaultHarnessSection />

      <Section title="License">
        {license.isLoading ? (
          <Skeleton className="h-32 rounded-xl" />
        ) : license.data ? (
          <LicenseDetails data={license.data} />
        ) : (
          <Callout tone="neutral">License status is unavailable.</Callout>
        )}
      </Section>
    </div>
  );
}

function LicenseDetails({ data }: { data: NonNullable<ReturnType<typeof useLicense>["data"]> }) {
  const { license, usage, warnings } = data;

  if (!license.valid) {
    return (
      <Callout tone="info" title="Community edition">
        No valid license is installed. Install an offline license secret to unlock enterprise features
        (see docs/development/licensing.md).
      </Callout>
    );
  }

  return (
    <div className="space-y-3">
      {warnings.length ? (
        <Callout tone="warning" title="License warnings">
          <ul className="list-disc pl-4">
            {warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        </Callout>
      ) : null}
      <div className="grid gap-4 lg:grid-cols-2">
        <Card className="p-4">
          <DescriptionList
            items={[
              { label: "Tier", value: <Badge variant="success">{titleCase(license.tier)}</Badge> },
              license.customer ? { label: "Customer", value: license.customer } : null,
              license.grace ? { label: "Status", value: <Badge variant="warning">Grace period</Badge> } : null,
              {
                label: "Expires",
                value: license.expiresAt ? formatDate(license.expiresAt) : "Never",
              },
              typeof license.daysRemaining === "number"
                ? { label: "Days remaining", value: license.daysRemaining }
                : null,
            ]}
          />
        </Card>
        <Card className="p-4">
          <h3 className="mb-3 text-[13px] font-medium text-muted-foreground">Usage</h3>
          <DescriptionList
            items={Object.entries(usage).map(([key, value]) => {
              const limitKey = key === "executionsToday" ? "executions_per_day" : key;
              const limit = license.limits[limitKey];
              return {
                label: humanize(key.replace(/([A-Z])/g, " $1").toLowerCase()),
                value: (
                  <span className="tabular">
                    {String(value)}
                    {limit ? <span className="font-normal text-muted-foreground"> of {String(limit)}</span> : null}
                  </span>
                ),
              };
            })}
          />
        </Card>
      </div>
      <div className="space-y-1.5">
        <h3 className="text-[13px] font-medium text-muted-foreground">Licensed features</h3>
        {license.features.length ? (
          <ul className="flex flex-wrap gap-1.5">
            {license.features.map((feature) => (
              <li key={feature}>
                <Badge variant="outline">{humanize(feature)}</Badge>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-[13px] text-muted-foreground">None.</p>
        )}
      </div>
    </div>
  );
}

function DefaultHarnessSection() {
  const queryClient = useQueryClient();
  const me = useMe();
  const canManage = me.data?.permissions.includes("settings:write") ?? false;
  const harnesses = useHarnesses();
  const settings = useQuery({
    queryKey: ["harness-settings"],
    queryFn: () => request<{ defaultHarness?: string | null }>("/api/v1/settings/harness"),
    enabled: canManage,
  });
  const save = useMutation({
    mutationFn: (defaultHarness: string) =>
      request("/api/v1/settings/harness", {
        method: "PUT",
        body: JSON.stringify({ defaultHarness: defaultHarness || null }),
      }),
    onSuccess: () => {
      toast.success("Default harness updated");
      void queryClient.invalidateQueries({ queryKey: ["harness-settings"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  if (!canManage) return null;

  return (
    <Section
      title="Default harness"
      description="Used for runs started by triggers and in the background, until a harness can be stored on the trigger itself."
    >
      <Card className="p-4">
        <Field label="Harness" htmlFor="default-harness" hint="Saved as soon as you choose.">
          <Select
            id="default-harness"
            className="sm:max-w-sm"
            value={settings.data?.defaultHarness ?? ""}
            disabled={settings.isLoading || save.isPending}
            onChange={(event) => save.mutate(event.target.value)}
          >
            <option value="">No default (require an explicit choice)</option>
            {(harnesses.data ?? [])
              .filter((harness) => harness.implementationStatus === "available")
              .map((harness) => (
              <option key={harness.kind} value={harness.kind}>
                {harness.name}
              </option>
            ))}
          </Select>
        </Field>
      </Card>
    </Section>
  );
}

/* ----------------------------------------------------------------------------
 * Audit log
 * ------------------------------------------------------------------------- */

function AuditTab() {
  const audit = useAuditLogs();

  return (
    <Section
      title="Recent activity"
      description="Who changed what in this organization, newest first."
    >
      {audit.isLoading ? (
        <ListSkeleton rows={5} />
      ) : audit.isError ? (
        <Callout tone="danger" title="Could not load the audit log">
          {errorMessage(audit.error)}
        </Callout>
      ) : audit.data?.items.length ? (
        <Card className="overflow-hidden">
          {audit.data.items.map((entry) => (
            <div
              key={entry.id}
              className="flex flex-col gap-0.5 border-b px-4 py-3 last:border-b-0 sm:flex-row sm:items-baseline sm:justify-between sm:gap-4 sm:px-5"
            >
              <div className="min-w-0">
                <p className="break-words text-sm font-medium" title={entry.action}>
                  {humanize(entry.action)}
                </p>
                <p className="break-words text-[13px] text-muted-foreground">
                  By {entry.actorEmail ?? titleCase(entry.actorType)}
                  {entry.targetType ? `, on ${titleCase(entry.targetType).toLowerCase()}` : ""}
                </p>
              </div>
              <time
                dateTime={entry.createdAt}
                className="shrink-0 text-[13px] text-muted-foreground"
              >
                {formatDateTime(entry.createdAt)}
              </time>
            </div>
          ))}
        </Card>
      ) : (
        <EmptyState title="No activity yet" description="Changes to agents, connections, roles and tokens appear here." />
      )}
    </Section>
  );
}
