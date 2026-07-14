export interface AppIconProps {
  size?: number;
  title?: string;
}

export function AppIcon({ size = 32, title }: AppIconProps) {
  return (
    <img
      alt={title ?? ""}
      aria-hidden={title ? undefined : true}
      className="app-icon"
      height={size}
      src="/branding/wuwa-ini-tool-mark.png"
      width={size}
    />
  );
}
