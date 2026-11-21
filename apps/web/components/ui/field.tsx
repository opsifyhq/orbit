import * as React from "react";
import { cn } from "@/lib/utils";
import { Label } from "@/components/ui/label";

const NATIVE_CONTROLS = new Set(["input", "select", "textarea"]);
/** Our primitives that render one native control and pass every prop on to it. */
const FORWARDING_CONTROLS = new Set(["Input", "Select", "Textarea"]);

/**
 * A single text-like control can be wired. A layout wrapper, a fragment, or a composite
 * (a picker, a segmented group) cannot: it would drop `id` and the label and hint would
 * point at nothing. Those keep whatever `htmlFor` the caller passes.
 */
function isControl(node: React.ReactNode): node is React.ReactElement<Record<string, unknown>> {
  if (!React.isValidElement(node)) return false;
  if (typeof node.type === "string") return NATIVE_CONTROLS.has(node.type);
  const name = (node.type as { displayName?: string }).displayName;
  return name !== undefined && FORWARDING_CONTROLS.has(name);
}

/**
 * Label + control + hint/error, wired together for assistive tech: the label names the
 * control, the hint or error describes it, and an error marks it invalid. Pass `htmlFor`
 * when the control sits inside a wrapper; a lone control child is wired automatically.
 */
export function Field({
  label,
  hint,
  error,
  optional,
  htmlFor,
  className,
  children,
}: {
  label: React.ReactNode;
  hint?: React.ReactNode;
  error?: React.ReactNode;
  optional?: boolean;
  htmlFor?: string;
  className?: string;
  children: React.ReactNode;
}) {
  const generatedId = React.useId();
  const messageId = `${generatedId}-message`;
  const hasMessage = Boolean(error || hint);

  let control = children;
  let controlId = htmlFor;
  if (React.Children.count(children) === 1 && isControl(children)) {
    const props = children.props;
    controlId = (props.id as string | undefined) ?? htmlFor ?? generatedId;
    const describedBy = [props["aria-describedby"], hasMessage ? messageId : null]
      .filter(Boolean)
      .join(" ");
    control = React.cloneElement(children, {
      id: controlId,
      "aria-describedby": describedBy || undefined,
      "aria-invalid": props["aria-invalid"] ?? (error ? true : undefined),
    });
  }

  return (
    <div className={cn("space-y-1.5", className)}>
      <Label htmlFor={controlId}>
        {label}
        {optional ? <span className="ml-1.5 font-normal text-muted-foreground">Optional</span> : null}
      </Label>
      {control}
      {error ? (
        <p id={messageId} className="text-[13px] text-destructive first-letter:uppercase" role="alert">
          {error}
        </p>
      ) : hint ? (
        <p id={messageId} className="text-[13px] text-muted-foreground">
          {hint}
        </p>
      ) : null}
    </div>
  );
}
