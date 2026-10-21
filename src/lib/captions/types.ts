/** Caption data, mirrored in src-tauri/src/style.rs. Copied from KreoKit Captions. */
export interface Word {
  start_ms: number;
  end_ms: number;
  text: string;
}

/** One caption segment of a clip, on the clip's clock (0 = the clip's in point). */
export interface Segment {
  id: number;
  start_ms: number;
  end_ms: number;
  text: string;
  words: Word[];
}

export type Position = "bottom" | "middle" | "top";
export type CaptionMode = "line" | "word_highlight" | "one_word";
export type Animation = "none" | "pop";

/** Sizes are in pixels for a 1080 px tall frame (a 608 × 1080 script for 9:16) and
 * scale with the video. */
export interface Style {
  font_family: string;
  font_size: number;
  color: string;
  highlight_color: string;
  outline_color: string;
  outline_width: number;
  position: Position;
  max_chars_per_line: number;
  max_lines: number;
  mode: CaptionMode;
  bold: boolean;
  uppercase: boolean;
  animation: Animation;
  /** Free position: the caption's anchor as fractions of the frame (0–1); null means the
   * preset spot for `position`, which also says which edge is the anchor
   * (top edge / centre / bottom edge). See position.ts. */
  pos_x: number | null;
  pos_y: number | null;
}
