import { useLayoutEffect, useRef, type ReactNode } from "react";

/** The row the grid counts in. A card spans as many as its height takes. */
const ROW = 8;

/**
 * Cards packed into as many columns as the screen fits, the way the Kiso
 * gallery lays out its components: each card spans only the rows its own
 * height needs, so a short card does not wait for the tallest one beside it.
 * A card with `data-size="wide"` takes two columns when there are two.
 */
export function Masonry({ children }: { children: ReactNode }) {
  const grid = useRef<HTMLDivElement>(null);

  // Every render, so a card that appears later is measured too.
  useLayoutEffect(() => {
    const root = grid.current;
    if (!root) return;
    const gap = parseFloat(getComputedStyle(root).columnGap) || 0;
    const fit = (card: HTMLElement) => {
      const height = card.getBoundingClientRect().height;
      card.style.setProperty("--masonry-rows", String(Math.ceil((height + gap) / ROW)));
    };
    const cards = Array.from(root.children).filter((child): child is HTMLElement => child instanceof HTMLElement);
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) fit(entry.target as HTMLElement);
    });
    for (const card of cards) {
      fit(card);
      observer.observe(card);
    }
    return () => observer.disconnect();
  });

  return (
    <div ref={grid} className="card-masonry">
      {children}
    </div>
  );
}
