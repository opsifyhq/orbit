import * as React from "react";
import { cn } from "@/lib/utils";

/**
 * Shared look for text-like controls (input, textarea, select).
 * 16px on phones: iOS zooms the page when a focused control is smaller than that.
 */
export const controlClass =
  "w-full rounded-md border border-input bg-card px-3 text-base text-foreground md:text-sm transition-colors placeholder:text-muted-foreground hover:border-muted-foreground focus-visible:border-ring focus-visible:outline-offset-0 disabled:cursor-not-allowed disabled:bg-muted disabled:opacity-60 aria-[invalid=true]:border-destructive";

const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input
      type={type}
      className={cn(
        controlClass,
        "flex h-9 py-1 file:border-0 file:bg-transparent file:text-sm file:font-medium",
        className,
      )}
      ref={ref}
      {...props}
    />
  ),
);
Input.displayName = "Input";

export { Input };
