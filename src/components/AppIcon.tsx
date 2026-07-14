import { useId } from "react";

export interface AppIconProps {
  size?: number;
  title?: string;
}

export function AppIcon({ size = 32, title }: AppIconProps) {
  const titleId = useId();

  return (
    <svg
      aria-hidden={title ? undefined : true}
      aria-labelledby={title ? titleId : undefined}
      className="app-icon"
      height={size}
      role={title ? "img" : undefined}
      viewBox="0 0 32 32"
      width={size}
      xmlns="http://www.w3.org/2000/svg"
    >
      {title ? <title id={titleId}>{title}</title> : null}
      <path d="M11 5H5v22h6M21 5h6v22h-6" />
      <path d="M10 9h12M16 9v14M12 23h8" />
    </svg>
  );
}
