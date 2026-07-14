import {
  cloneElement,
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

export function Tooltip({ children, label }: TooltipProps) {
  const [isOpen, setIsOpen] = useState(false);
  const tooltipId = useId();

  const trigger = cloneElement(children, {
    "aria-describedby": isOpen ? tooltipId : undefined,
    onBlur: (event) => {
      children.props.onBlur?.(event);
      setIsOpen(false);
    },
    onClick: (event) => {
      children.props.onClick?.(event);
      setIsOpen(true);
    },
    onFocus: (event) => {
      children.props.onFocus?.(event);
      setIsOpen(true);
    },
    onKeyDown: (event) => {
      children.props.onKeyDown?.(event);
      if (event.key === "Escape") {
        setIsOpen(false);
        event.currentTarget.focus();
      }
    },
    onMouseEnter: (event) => {
      children.props.onMouseEnter?.(event);
      setIsOpen(true);
    },
    onMouseLeave: (event) => {
      children.props.onMouseLeave?.(event);
      setIsOpen(false);
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
