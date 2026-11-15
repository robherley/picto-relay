import type { CSSProperties, ReactNode } from "react";
import { boxBorder, lighten, tagBorder } from "./borders";

interface Props {
  name: string;
  /** The sender's color, as `#rrggbb`. */
  tone: string;
  className?: string;
  /** Makes the name tag a button. */
  onNameClick?: () => void;
  children: ReactNode;
}

/**
 * A PictoChat message: a box with the sender's name tag in its corner, both
 * tinted with the sender's profile color as on a DS.
 */
export function MessageBox({ name, tone, className = "", onNameClick, children }: Props) {
  const style = {
    "--tone": tone,
    borderImageSource: boxBorder(tone, lighten(tone, 0.6)),
  } as CSSProperties;
  return (
    <div className={`pictochat-message message ${className}`} style={style}>
      <header
        style={{ borderImageSource: tagBorder(tone) }}
        role={onNameClick ? "button" : undefined}
        tabIndex={onNameClick ? 0 : undefined}
        onClick={onNameClick}
        onKeyDown={(event) => {
          if (onNameClick && (event.key === "Enter" || event.key === " ")) onNameClick();
        }}
      >
        {name}
      </header>
      {children}
    </div>
  );
}
