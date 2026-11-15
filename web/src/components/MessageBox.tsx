import type { CSSProperties, ReactNode } from "react";
import { boxBorder, lighten, tagBorder } from "./borders";

interface Props {
  name: string;
  /** The sender's color, as `#rrggbb`. */
  tone: string;
  className?: string;
  children: ReactNode;
}

/**
 * A PictoChat message: a box with the sender's name tag in its corner, both
 * tinted with the sender's profile color as on a DS.
 */
export function MessageBox({ name, tone, className = "", children }: Props) {
  const style = {
    "--tone": tone,
    borderImageSource: boxBorder(tone, lighten(tone, 0.6)),
  } as CSSProperties;
  return (
    <div className={`pictochat-message message ${className}`} style={style}>
      <header style={{ borderImageSource: tagBorder(tone) }}>{name}</header>
      {children}
    </div>
  );
}
