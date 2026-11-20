"use client";

import type { ReactNode } from "react";

import {
  SectionCard as SharedSectionCard,
  SectionPills as SharedSectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";

import {
  SECTION_DESCRIPTIONS,
  SECTION_ORDER,
  SECTION_TITLES,
  type EditorIssue,
  type SectionId,
} from "./editor-sections";

type Summary = Record<SectionId, { value: string; missing?: boolean }>;

const NAV_LABEL = "Worker profile sections";

function navSections(issues: EditorIssue[], summaries?: Summary): NavSection<SectionId>[] {
  return SECTION_ORDER.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summaries?.[id].value,
    missing: summaries?.[id].missing,
    problems: issues.filter((issue) => issue.section === id).length,
  }));
}

/** Desktop: every section with its current value, so the nav is also the summary. */
export function SectionNav({
  active,
  onSelect,
  summaries,
  issues,
}: {
  active: SectionId;
  onSelect: (id: SectionId) => void;
  summaries: Summary;
  issues: EditorIssue[];
}) {
  return (
    <SectionRail
      label={NAV_LABEL}
      sections={navSections(issues, summaries)}
      active={active}
      onGo={onSelect}
    />
  );
}

/** Phone and tablet: the sections as a scrollable row of steps. */
export function SectionPills({
  active,
  onSelect,
  issues,
}: {
  active: SectionId;
  onSelect: (id: SectionId) => void;
  issues: EditorIssue[];
}) {
  return (
    <SharedSectionPills
      label={NAV_LABEL}
      sections={navSections(issues)}
      active={active}
      onGo={onSelect}
      className="mb-4"
    />
  );
}

/** The section on screen: a titled card that ends with Back and Next. */
export function SectionCard({
  id,
  active,
  onGo,
  keepMounted,
  children,
}: {
  id: SectionId;
  active: SectionId;
  onGo: (id: SectionId) => void;
  /** Hide instead of unmounting, for a section whose own state Save still reads. */
  keepMounted?: boolean;
  children: ReactNode;
}) {
  return (
    <SharedSectionCard
      id={id}
      active={active}
      title={SECTION_TITLES[id]}
      description={SECTION_DESCRIPTIONS[id]}
      onGo={onGo}
      keepMounted={keepMounted}
      {...sectionNeighbours(SECTION_ORDER, SECTION_TITLES, id)}
    >
      {children}
    </SharedSectionCard>
  );
}
