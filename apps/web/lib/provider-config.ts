/** Provider configs are JSON objects; the form only renders a subset of their keys. */
export type ProviderConfig = Record<string, unknown>;

type Section = Record<string, unknown>;

const asSection = (value: unknown): Section | undefined =>
  value && typeof value === "object" && !Array.isArray(value) ? (value as Section) : undefined;

/**
 * Apply the fields a provider form renders to the config stored on the server.
 *
 * Editing replaces only those fields: extra headers, request params, compat flags
 * and harness settings that the form does not show must survive a save. A create
 * has no stored config, so the rendered config passes through unchanged.
 */
export function mergeProviderConfig(
  existing: ProviderConfig | null | undefined,
  rendered: ProviderConfig,
): ProviderConfig {
  if (!existing) return rendered;

  const merged: ProviderConfig = { ...existing, ...rendered };
  // These two sections are merged rather than replaced: they hold flags the form
  // does not render and settings from other tools.
  const compat = { ...asSection(existing.compat), ...asSection(rendered.compat) };
  if (existing.compat || rendered.compat) merged.compat = compat;
  const settings = { ...asSection(existing.settings), ...asSection(rendered.settings) };
  if (existing.settings || rendered.settings) merged.settings = settings;
  return merged;
}
