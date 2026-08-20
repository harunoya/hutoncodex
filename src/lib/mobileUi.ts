import { useEffect, useState, type RefObject } from "react";

const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "a[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export type ViewportMetrics = {
  compact: boolean;
  phone: boolean;
  keyboardOpen: boolean;
};

export function useViewportMetrics(): ViewportMetrics {
  const [metrics, setMetrics] = useState<ViewportMetrics>(() => readViewportMetrics());

  useEffect(() => {
    const viewport = window.visualViewport;
    const sync = () => {
      const next = readViewportMetrics();
      setMetrics(next);
      document.documentElement.style.setProperty(
        "--app-viewport-height",
        `${Math.round(viewport?.height ?? window.innerHeight)}px`,
      );
      document.documentElement.style.setProperty(
        "--app-viewport-offset-top",
        `${Math.round(viewport?.offsetTop ?? 0)}px`,
      );
      document.body.classList.toggle("keyboard-open", next.keyboardOpen);
    };

    sync();
    viewport?.addEventListener("resize", sync);
    viewport?.addEventListener("scroll", sync);
    window.addEventListener("resize", sync);
    window.addEventListener("orientationchange", sync);
    return () => {
      viewport?.removeEventListener("resize", sync);
      viewport?.removeEventListener("scroll", sync);
      window.removeEventListener("resize", sync);
      window.removeEventListener("orientationchange", sync);
      document.body.classList.remove("keyboard-open");
      document.documentElement.style.removeProperty("--app-viewport-height");
      document.documentElement.style.removeProperty("--app-viewport-offset-top");
    };
  }, []);

  return metrics;
}

export function useModalFocus(
  containerRef: RefObject<HTMLElement | null>,
  active: boolean,
) {
  useEffect(() => {
    if (!active) return;
    const previous = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const container = containerRef.current;
    if (!container) return;

    const focusFirst = () => {
      const first = visibleFocusableElements(container)[0];
      (first ?? container).focus({ preventScroll: true });
    };
    const focusTimer = window.setTimeout(focusFirst, 0);
    const trapFocus = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const focusable = visibleFocusableElements(container);
      if (!focusable.length) {
        event.preventDefault();
        container.focus({ preventScroll: true });
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", trapFocus);
    return () => {
      window.clearTimeout(focusTimer);
      document.removeEventListener("keydown", trapFocus);
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, [active, containerRef]);
}

export function shouldSubmitComposer(event: {
  key: string;
  shiftKey: boolean;
  isComposing?: boolean;
  keyCode?: number;
}) {
  return event.key === "Enter"
    && !event.shiftKey
    && !event.isComposing
    && event.keyCode !== 229;
}

export function isNearScrollBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  threshold = 72,
) {
  return scrollHeight - scrollTop - clientHeight <= threshold;
}

function readViewportMetrics(): ViewportMetrics {
  const width = window.visualViewport?.width ?? window.innerWidth;
  const viewportHeight = window.visualViewport?.height ?? window.innerHeight;
  const keyboardDelta = Math.max(0, window.innerHeight - viewportHeight);
  const landscapePhone = viewportHeight <= 500 && width <= 932;
  return {
    compact: width <= 840 || landscapePhone,
    phone: width <= 600 || landscapePhone,
    keyboardOpen: keyboardDelta >= 120,
  };
}

function visibleFocusableElements(container: HTMLElement) {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR))
    .filter((element) => !element.hidden && element.getAttribute("aria-hidden") !== "true");
}
