import type { ButtonHTMLAttributes, ReactNode } from "react";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  children: ReactNode;
  variant?: "primary" | "secondary" | "danger";
}

export function Button({
  children,
  className = "",
  type = "button",
  variant = "secondary",
  ...props
}: ButtonProps) {
  const classes = ["button", `button--${variant}`, className]
    .filter(Boolean)
    .join(" ");

  return (
    <button className={classes} type={type} {...props}>
      {children}
    </button>
  );
}
