import { useId, useState, type ReactNode } from "react";

export interface AccordionProps {
  children: ReactNode;
  defaultOpen?: boolean;
  title: ReactNode;
}

export function Accordion({
  children,
  defaultOpen = false,
  title,
}: AccordionProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen);
  const buttonId = useId();
  const panelId = useId();

  return (
    <section className="accordion">
      <h2 className="accordion__heading">
        <button
          aria-controls={panelId}
          aria-expanded={isOpen}
          className="accordion__trigger"
          id={buttonId}
          onClick={() => setIsOpen((open) => !open)}
          type="button"
        >
          <span>{title}</span>
          <span aria-hidden="true" className="accordion__indicator">
            {isOpen ? "−" : "+"}
          </span>
        </button>
      </h2>
      <div
        aria-labelledby={buttonId}
        className="accordion__panel"
        hidden={!isOpen}
        id={panelId}
        role="region"
      >
        {children}
      </div>
    </section>
  );
}
