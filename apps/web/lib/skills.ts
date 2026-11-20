import { isOrbitApiError } from "@orbit/api-client";

import type { SkillInput } from "@/components/skills/skill-form";
import { api } from "@/lib/api";
import { unwrap } from "@/lib/queries";

export async function createSkill(input: SkillInput) {
  return unwrap(await api.POST("/api/v1/skills", { body: input }));
}

export async function updateSkill(id: string, input: SkillInput) {
  const { name, description, content, files } = input;
  return unwrap(
    await api.PATCH("/api/v1/skills/{skill_id}", {
      params: { path: { skill_id: id } },
      body: { name, description, content, files },
    }),
  );
}

export async function deleteSkill(id: string) {
  return unwrap(await api.DELETE("/api/v1/skills/{skill_id}", { params: { path: { skill_id: id } } }));
}

/** Field-level validation details from a failed save, for the form to place per section. */
export function validationDetails(error: unknown): Array<{ path: string; message: string }> {
  return isOrbitApiError(error) ? (error.error.details ?? []) : [];
}
