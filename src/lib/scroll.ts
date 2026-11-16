/** True when the view is at (or within a few lines of) the bottom, so new text should be followed. */
export function stickToBottom(scrollTop: number, clientHeight: number, scrollHeight: number, slack = 80): boolean {
  return scrollHeight - (scrollTop + clientHeight) <= slack;
}
