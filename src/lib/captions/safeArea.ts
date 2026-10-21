/**
 * Parts of a vertical (9:16) video that TikTok, Instagram Reels and YouTube Shorts cover
 * with their own interface (header, captions/description and buttons). A conservative
 * blend of the three; fractions of the frame.
 */
export const SAFE_AREA = { top: 0.1, bottom: 0.22, right: 0.14 };

/** The safe-area overlay applies to vertical videos only. */
export function isVertical(size: { width: number; height: number } | null): boolean {
  return !!size && size.height > size.width;
}

const KEY = "kreokit.showSafeArea";

/** The "Show social safe area" switch, remembered on this computer. */
export function loadShowSafeArea(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function saveShowSafeArea(show: boolean): void {
  try {
    localStorage.setItem(KEY, show ? "1" : "0");
  } catch {
    // Storage unavailable: the switch just starts off next time.
  }
}
