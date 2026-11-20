/** A JSON-schema property, narrowed to what the connection form needs to coerce a value. */
export type ConfigProperty = { type?: string };

export type ConfigFieldResult = {
  configText: string;
  error?: string;
};

/**
 * Change one key in a raw JSON config, leaving every other key as it was.
 *
 * On invalid JSON the original text is returned with an error instead of being
 * replaced with `{}`; otherwise typing in a schema field would silently erase
 * the rest of the config.
 */
export function setConfigField(
  configText: string,
  key: string,
  raw: string,
  property?: ConfigProperty,
): ConfigFieldResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(configText || "{}");
  } catch {
    return { configText, error: "Config is not valid JSON. Fix it before changing a field." };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { configText, error: "Config must be a JSON object before changing a field." };
  }

  const config = { ...(parsed as Record<string, unknown>) };
  if (raw === "") {
    delete config[key];
  } else if (property?.type === "number" || property?.type === "integer") {
    const value = Number(raw);
    if (!Number.isFinite(value)) return { configText, error: "Enter a valid number for this field." };
    config[key] = value;
  } else if (property?.type === "boolean") {
    config[key] = raw === "true";
  } else if (property?.type === "object" || property?.type === "array") {
    try {
      config[key] = JSON.parse(raw) as unknown;
    } catch {
      return { configText, error: "Enter valid JSON for this field." };
    }
  } else {
    config[key] = raw;
  }

  return { configText: JSON.stringify(config, null, 2) };
}
