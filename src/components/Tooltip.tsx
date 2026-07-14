import {
  cloneElement,
  useEffect,
  useId,
  useState,
  type FocusEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
} from "react";

interface TooltipTriggerProps {
  "aria-describedby"?: string;
  onBlur?: (event: FocusEvent<HTMLElement>) => void;
  onClick?: (event: MouseEvent<HTMLElement>) => void;
  onFocus?: (event: FocusEvent<HTMLElement>) => void;
  onKeyDown?: (event: KeyboardEvent<HTMLElement>) => void;
  onMouseEnter?: (event: MouseEvent<HTMLElement>) => void;
  onMouseLeave?: (event: MouseEvent<HTMLElement>) => void;
}

export interface TooltipProps {
  children: ReactElement<TooltipTriggerProps>;
  label: string;
}

interface OpenReasons {
  click: boolean;
  focus: boolean;
  hover: boolean;
}

const closedReasons: OpenReasons = {
  click: false,
  focus: false,
  hover: false,
};

export function Tooltip({ children, label }: TooltipProps) {
  const [openReasons, setOpenReasons] = useState<OpenReasons>(closedReasons);
  const tooltipId = useId();
  const isOpen = openReasons.click || openReasons.focus || openReasons.hover;

  useEffect(() => {
    if (!isOpen) {
      return;
    }

    function handleKeyDown(event: globalThis.KeyboardEvent) {
      if (event.key === "Escape") {
        setOpenReasons(closedReasons);
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isOpen]);

  const describedBy = [
    children.props["aria-describedby"],
    isOpen ? tooltipId : undefined,
  ]
    .filter(Boolean)
    .join(" ");

  const trigger = cloneElement(children, {
    "aria-describedby": describedBy || undefined,
    onBlur: (event) => {
      children.props.onBlur?.(event);
      setOpenReasons((current) => ({ ...current, focus: false }));
    },
    onClick: (event) => {
      children.props.onClick?.(event);
      setOpenReasons((current) => ({ ...current, click: !current.click }));
    },
    onFocus: (event) => {
      children.props.onFocus?.(event);
      setOpenReasons((current) => ({ ...current, focus: true }));
    },
    onKeyDown: (event) => {
      children.props.onKeyDown?.(event);
    },
    onMouseEnter: (event) => {
      children.props.onMouseEnter?.(event);
      setOpenReasons((current) => ({ ...current, hover: true }));
    },
    onMouseLeave: (event) => {
      children.props.onMouseLeave?.(event);
      setOpenReasons((current) => ({ ...current, hover: false }));
    },
  });

  return (
    <span className="tooltip">
      {trigger}
      {isOpen ? (
        <span className="tooltip__content" id={tooltipId} role="tooltip">
          {label}
        </span>
      ) : null}
    </span>
  );
}
