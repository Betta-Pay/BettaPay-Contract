import { toast } from 'sonner';

/**
 * Metadata representing an interactive call-to-action button inside a toast.
 */
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

/**
 * Custom React hook wrapping the `sonner` library to enforce a standard notification strategy
 * across all features, ensuring consistent positioning, durations, and interactive action behaviors.
 */
export function useNotify() {
  // Global configuration defaults forcing all visible toasts to open at the bottom-right screen coordinate.
  const globalDefaults = {
    position: 'bottom-right' as const,
  };

  /**
   * Surfaces a success notification.
   * For user-initiated loops (saves, creations, deletes).
   * Fixed duration: 3s (3000ms).
   */
  const success = (message: string, context?: NotificationContext) => {
    toast.success(message, {
      ...globalDefaults,
      duration: 3000,
      description: context?.description,
      action: context?.action
        ? {
            label: context.action.label,
            onClick: context.action.onClick,
          }
        : undefined,
    });
  };

  /**
   * Surfaces an error notification.
   * For recoverable problems (API faults, parsing errors).
   * Fixed duration: 5s (5000ms).
   */
  const error = (message: string, context?: NotificationContext) => {
    toast.error(message, {
      ...globalDefaults,
      duration: 5000,
      description: context?.description,
      action: context?.action
        ? {
            label: context.action.label,
            onClick: context.action.onClick,
          }
        : undefined,
    });
  };

  /**
   * Surfaces an info notification.
   * For background system milestones (session expirations, network alerts).
   * Fixed duration: 4s (4000ms).
   */
  const info = (message: string, context?: NotificationContext) => {
    toast.info(message, {
      ...globalDefaults,
      duration: 4000,
      description: context?.description,
      action: context?.action
        ? {
            label: context.action.label,
            onClick: context.action.onClick,
          }
        : undefined,
    });
  };

  /**
   * Tracks background tasks silently without flashing banners.
   * Explicit empty handler code block.
   */
  const silent = (message: string, context?: NotificationContext) => {
    // Explicit empty handler code block tracking background tasks without flashing banners
  };

  return {
    success,
    error,
    info,
    silent,
  };
}
