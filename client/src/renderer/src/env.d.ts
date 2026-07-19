import type { DesktopBridge } from '../../shared/types';

declare global {
  interface Window {
    gaterust: DesktopBridge;
  }
}

export {};
