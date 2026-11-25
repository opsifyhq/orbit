"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Bot, CornerDownLeft, Monitor, Moon, Play, Plus, Search, Sun, type LucideIcon } from "lucide-react";

import { DialogOverlay, DialogPortal, useRestoreFocus } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/kbd";
import { AgentStatusBadge } from "@/components/ui/status-badge";
import { NAV_GROUPS } from "@/components/shell/nav";
import { useAllAgents } from "@/lib/queries";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";

interface Command {
  id: string;
  group: string;
  label: string;
  hint?: string;
  keywords?: string;
  icon: LucideIcon;
  trailing?: React.ReactNode;
  run: () => void;
}

const PaletteContext = React.createContext<{ open: () => void } | null>(null);

/** The palette, or null outside the signed-in shell (a page that may render either way). */
export function useOptionalCommandPalette() {
  return React.useContext(PaletteContext);
}

export function useCommandPalette() {
  const context = React.useContext(PaletteContext);
  if (!context) throw new Error("useCommandPalette must be used inside <CommandPaletteProvider>");
  return context;
}

function matches(command: Command, terms: string[]) {
  const haystack = `${command.label} ${command.keywords ?? ""} ${command.group}`.toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

/**
 * ⌘K / Ctrl+K: jump to any page or agent, or start the common actions, without
 * walking the sidebar. One input, arrow keys, Enter.
 */
export function CommandPaletteProvider({ children }: { children: React.ReactNode }) {
  const router = useRouter();
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [cursor, setCursor] = React.useState(0);
  const listRef = React.useRef<HTMLDivElement>(null);
  const { setMode } = useTheme();
  const focus = useRestoreFocus();
  const agents = useAllAgents({ status: "current", enabled: open });

  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen((value) => {
          if (value) return false;
          // Never stack on top of another dialog (a confirmation, a form).
          return !document.querySelector('[role="dialog"], [role="alertdialog"]');
        });
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  React.useEffect(() => {
    if (open) {
      setQuery("");
      setCursor(0);
    }
  }, [open]);

  const commands = React.useMemo<Command[]>(() => {
    const go = (href: string) => () => router.push(href);
    const actions: Command[] = [
      { id: "act-work", group: "Actions", label: "Start work", keywords: "run execute task new", icon: Play, run: go("/work") },
      { id: "act-agent", group: "Actions", label: "New agent", keywords: "create add", icon: Plus, run: go("/agents/new") },
    ];
    const pages: Command[] = NAV_GROUPS.flatMap((group) =>
      group.items.map((item) => ({
        id: `nav-${item.href}`,
        group: "Go to",
        label: item.label,
        hint: group.label ?? undefined,
        keywords: item.keywords,
        icon: item.icon,
        run: go(item.href),
      })),
    );
    const agentCommands: Command[] = (agents.data?.items ?? []).map((agent) => ({
      id: `agent-${agent.id}`,
      group: "Agents",
      label: agent.name,
      keywords: `${agent.description ?? ""} ${agent.harness ?? ""}`,
      icon: Bot,
      trailing: <AgentStatusBadge status={agent.status} className="text-xs" />,
      run: go(`/agents/${agent.id}`),
    }));
    const theme: Command[] = [
      { id: "theme-light", group: "Theme", label: "Use light theme", keywords: "appearance", icon: Sun, run: () => setMode("light") },
      { id: "theme-dark", group: "Theme", label: "Use dark theme", keywords: "appearance night", icon: Moon, run: () => setMode("dark") },
      { id: "theme-system", group: "Theme", label: "Follow system theme", keywords: "appearance auto", icon: Monitor, run: () => setMode("system") },
    ];
    return [...actions, ...pages, ...agentCommands, ...theme];
  }, [agents.data?.items, router, setMode]);

  const results = React.useMemo(() => {
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!terms.length) return commands.filter((command) => command.group !== "Theme");
    return commands.filter((command) => matches(command, terms));
  }, [commands, query]);

  React.useEffect(() => {
    setCursor(0);
  }, [query]);

  // Results also change when agents finish loading; keep the cursor on a real row.
  React.useEffect(() => {
    setCursor((value) => Math.min(value, Math.max(0, results.length - 1)));
  }, [results.length]);

  React.useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${cursor}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  function run(command: Command | undefined) {
    if (!command) return;
    setOpen(false);
    command.run();
  }

  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setCursor((value) => Math.min(value + 1, results.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setCursor((value) => Math.max(value - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      run(results[cursor]);
    }
  }

  const context = React.useMemo(() => ({ open: () => setOpen(true) }), []);
  let lastGroup = "";

  return (
    <PaletteContext.Provider value={context}>
      {children}
      <DialogPrimitive.Root open={open} onOpenChange={setOpen}>
        <DialogPortal>
          <DialogOverlay />
          <DialogPrimitive.Content
            onOpenAutoFocus={focus.remember}
            onCloseAutoFocus={focus.restore}
            aria-describedby={undefined}
            className="fixed left-1/2 top-[12vh] z-50 flex max-h-[min(34rem,76vh)] w-[calc(100%-1.5rem)] max-w-xl -translate-x-1/2 flex-col overflow-hidden rounded-xl border bg-popover shadow-overlay data-[state=open]:animate-overlay-in"
          >
            <DialogPrimitive.Title className="sr-only">Search and commands</DialogPrimitive.Title>
            <div className="flex items-center gap-2.5 border-b px-4">
              <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
              <input
                autoFocus
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={onKeyDown}
                placeholder="Search pages, agents and actions"
                // 16px on phones: iOS zooms the page when a focused field is smaller.
                className="h-12 w-full bg-transparent text-base outline-none placeholder:text-muted-foreground md:text-[15px]"
                role="combobox"
                aria-label="Search pages, agents and actions"
                aria-autocomplete="list"
                aria-expanded
                aria-controls="command-results"
                aria-activedescendant={results[cursor] ? `command-${results[cursor].id}` : undefined}
              />
              <Kbd className="hidden sm:inline-flex">Esc</Kbd>
            </div>
            <p className="sr-only" role="status">
              {results.length === 0 ? `Nothing matches ${query}` : `${results.length} results`}
            </p>
            <div
              ref={listRef}
              id="command-results"
              role="listbox"
              aria-label="Results"
              className="min-h-0 flex-1 overflow-y-auto p-1.5"
            >
              {results.length === 0 ? (
                <p role="presentation" className="px-3 py-8 text-center text-sm text-muted-foreground">
                  Nothing matches “{query}”.
                </p>
              ) : (
                results.map((command, index) => {
                  const heading = command.group !== lastGroup ? command.group : null;
                  lastGroup = command.group;
                  const active = index === cursor;
                  return (
                    <React.Fragment key={command.id}>
                      {heading ? (
                        // A visual divider only: a listbox may contain nothing but options.
                        <p
                          role="presentation"
                          className="px-2.5 pb-1 pt-2.5 text-xs font-medium text-muted-foreground first:pt-1"
                        >
                          {heading}
                        </p>
                      ) : null}
                      <button
                        type="button"
                        id={`command-${command.id}`}
                        role="option"
                        aria-selected={active}
                        // The input keeps focus and points at the active option; the
                        // options themselves are not tab stops.
                        tabIndex={-1}
                        data-index={index}
                        onMouseMove={() => setCursor(index)}
                        onClick={() => run(command)}
                        className={cn(
                          "flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-sm",
                          active ? "bg-accent text-accent-foreground" : "text-foreground",
                        )}
                      >
                        <command.icon className={cn("h-4 w-4 shrink-0", active ? "" : "text-muted-foreground")} />
                        <span className="min-w-0 flex-1 truncate">{command.label}</span>
                        {command.trailing}
                        {command.hint ? (
                          <span className="shrink-0 text-xs text-muted-foreground">{command.hint}</span>
                        ) : null}
                        {active ? <CornerDownLeft className="h-3.5 w-3.5 shrink-0 opacity-60" /> : null}
                      </button>
                    </React.Fragment>
                  );
                })
              )}
            </div>
          </DialogPrimitive.Content>
        </DialogPortal>
      </DialogPrimitive.Root>
    </PaletteContext.Provider>
  );
}
