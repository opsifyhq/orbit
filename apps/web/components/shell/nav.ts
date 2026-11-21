import {
  Activity,
  BookOpen,
  Bot,
  Boxes,
  Cable,
  Cpu,
  Gauge,
  LifeBuoy,
  Megaphone,
  Server,
  Settings,
  ShieldCheck,
  Terminal,
  UsersRound,
  Workflow,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  href: string;
  label: string;
  icon: LucideIcon;
  /** Extra words the command palette matches on. */
  keywords?: string;
}

export interface NavGroup {
  id: string;
  label: string | null;
  items: NavItem[];
}

/**
 * Navigation follows the operator's questions, in order of how often they are asked:
 * what is happening → what do my agents look like → what can they reach → where do
 * they run → who may do what.
 */
export const NAV_GROUPS: NavGroup[] = [
  {
    id: "operate",
    label: null,
    items: [
      { href: "/", label: "Overview", icon: Gauge, keywords: "dashboard home status" },
      { href: "/executions", label: "Executions", icon: Activity, keywords: "runs history logs" },
      { href: "/approvals", label: "Approvals", icon: ShieldCheck, keywords: "review pending human" },
    ],
  },
  {
    id: "build",
    label: "Build",
    items: [
      { href: "/agents", label: "Agents", icon: Bot, keywords: "definitions studio" },
      { href: "/agent-teams", label: "Agent teams", icon: Workflow, keywords: "pipeline orchestration" },
      { href: "/skills", label: "Skills", icon: BookOpen, keywords: "skill.md packages" },
    ],
  },
  {
    id: "connect",
    label: "Connect",
    items: [
      { href: "/connections", label: "Connectors", icon: Cable, keywords: "connections integrations github gitlab telegram webhook" },
      { href: "/models", label: "Model providers", icon: Boxes, keywords: "models llm openai gateway" },
      { href: "/outputs", label: "Outputs", icon: Megaphone, keywords: "notifications slack mattermost telegram webhook" },
    ],
  },
  {
    id: "infrastructure",
    label: "Infrastructure",
    items: [
      { href: "/workers", label: "Workers", icon: Server, keywords: "profiles launches docker kubernetes" },
      { href: "/harnesses", label: "Harnesses", icon: Cpu, keywords: "adapters opencode pi mock claude" },
      { href: "/environments", label: "Environments", icon: Terminal, keywords: "env variables environments" },
    ],
  },
  {
    id: "admin",
    label: "Admin",
    items: [
      { href: "/teams", label: "Access teams", icon: UsersRound, keywords: "members rbac permissions sharing" },
      { href: "/settings", label: "Settings", icon: Settings, keywords: "tokens oidc sso audit license identity" },
      { href: "/documentation", label: "Documentation", icon: LifeBuoy, keywords: "docs handbook help guide manual" },
    ],
  },
];

export const NAV_ITEMS: NavItem[] = NAV_GROUPS.flatMap((group) => group.items);

/** The four destinations a phone user needs within thumb reach. */
export const MOBILE_PRIMARY = ["/", "/executions", "/approvals", "/agents"];

export function isActivePath(pathname: string, href: string) {
  if (href === "/") return pathname === "/";
  return pathname === href || pathname.startsWith(`${href}/`);
}

/** The "Start work" composer. Exact match: "/workers" is a different page. */
export function isWorkPath(pathname: string) {
  return pathname === "/work" || pathname.startsWith("/work/");
}

export function currentNavItem(pathname: string): NavItem | undefined {
  if (isWorkPath(pathname)) return undefined;
  return NAV_ITEMS.filter((item) => isActivePath(pathname, item.href)).at(-1);
}

export function pageLabel(pathname: string): string {
  if (isWorkPath(pathname)) return "Start work";
  return currentNavItem(pathname)?.label ?? "ORBIT";
}
