/** Whether the person asked the system for less motion. False on the server. */
export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/**
 * The `behavior` for scripted scrolling: smooth, unless reduced motion is on. The CSS
 * `scroll-behavior` rule does not apply to `scrollIntoView({ behavior: "smooth" })` (#448).
 */
export function scrollBehavior(): ScrollBehavior {
  return prefersReducedMotion() ? "auto" : "smooth";
}
