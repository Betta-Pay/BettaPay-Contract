export interface ToastAction {
  /** The text label displayed on the action button. */
  label: string;
  /** The click handler callback triggered when the action button is pressed. */
  onClick: () => void;
}

/**
 * Optional context block configuration accepted by useNotify handlers.
 */
export interface NotificationContext {
  /** Optional interactive action metadata for CTA rendering (e.g., Undo, Retry) */
  action?: ToastAction;
  /** Optional longer text explanation rendered beneath the main toast message */
  description?: string;
  /** Allow arbitrary telemetry or application context properties */
  [key: string]: unknown;
}
