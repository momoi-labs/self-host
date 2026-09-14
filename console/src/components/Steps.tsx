import type { ReactNode } from "react";

/**
 * A numbered form, read top to bottom. Building a custom image and creating a
 * virtual machine are the same shape of task: name the thing, choose its
 * tools, say how to verify it. Numbering says they happen in that order.
 */
export function Steps({ children }: { children: ReactNode }) {
  return <ol className="form-steps">{children}</ol>;
}

export function Step({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <li>
      <p className="t-caps">{title}</p>
      {children}
    </li>
  );
}
