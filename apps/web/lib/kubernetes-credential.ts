/**
 * A Kubernetes connection stores one credential, and what it means depends on `authMode`:
 * a service-account token or a kubeconfig. The form keeps one credential field for both, so
 * these rules keep a value typed for one mode from being saved as the other.
 */

export type KubernetesAuthMode = "service_account" | "kubeconfig";

export function kubernetesAuthMode(config: Record<string, unknown> | null | undefined): KubernetesAuthMode {
  return config?.authMode === "kubeconfig" ? "kubeconfig" : "service_account";
}

/**
 * The problem with saving this credential, or null. When an existing connection moves to
 * another mode, the stored credential belongs to the old mode, so a new one is required.
 */
export function kubernetesCredentialIssue({
  editing,
  storedMode,
  mode,
  credential,
}: {
  editing: boolean;
  storedMode: KubernetesAuthMode;
  mode: KubernetesAuthMode;
  credential: string;
}): string | null {
  if (!editing || storedMode === mode || credential.trim()) return null;
  return mode === "kubeconfig"
    ? "The authentication changed to kubeconfig: load or paste a kubeconfig. The stored token is not a kubeconfig."
    : "The authentication changed to service account: enter its token. The stored kubeconfig is not a token.";
}
