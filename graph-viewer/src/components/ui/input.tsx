import * as React from "react";
import { cn } from "@/lib/utils";

const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, type, ...props }, ref) => (
    <input
      type={type}
      className={cn(
        "flex h-8 w-full rounded-lg border border-white/[0.06] bg-white/[0.04] px-3 py-1.5 text-[13px] text-foreground placeholder-foreground/25 outline-none focus:border-primary/40 focus:bg-white/[0.06] transition-all",
        className,
      )}
      ref={ref}
      {...props}
    />
  ),
);
Input.displayName = "Input";

export { Input };
