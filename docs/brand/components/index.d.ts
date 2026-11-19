import type * as React from 'react';

export interface LogoProps {
  /** Height of the mark in px. Below 24 the simplified small mark is drawn. Default 32. */
  size?: number;
  /** 'horizontal' = mark + wordmark (default); 'mark' = the mark alone. */
  lockup?: 'horizontal' | 'mark';
  /** 'default' paints the equivalence bar in trace; 'mono' paints every bar in the current text color. */
  tone?: 'default' | 'mono';
  className?: string;
}
export declare function Logo(props: LogoProps): React.ReactElement;

export interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'quiet';
  size?: 'md' | 'sm';
}
export declare function Button(props: ButtonProps): React.ReactElement;

export type StatusKind = 'accepted' | 'pass' | 'fail' | 'in-progress' | 'partial' | 'early' | 'planned';
export interface StatusProps {
  status: StatusKind;
  /** Replaces the default word. Keep it a status word, set in the same case as the default. */
  children?: React.ReactNode;
  className?: string;
}
export declare function Status(props: StatusProps): React.ReactElement;

export interface Stage { n: number | string; scope: string; status: StatusKind; note?: string }
export interface StageTableProps { stages: Stage[]; caption?: string }
export declare function StageTable(props: StageTableProps): React.ReactElement;

export interface VMRowProps {
  name: string;
  arch?: string;
  cpus?: number;
  memory?: string;
  state: 'running' | 'stopped' | 'paused' | 'error';
  /** Error text for state 'error': what stopped the guest and where, e.g. a fault and its RIP. */
  detail?: string;
  actions?: React.ReactNode;
  className?: string;
}
export declare function VMRow(props: VMRowProps): React.ReactElement;

declare global {
  interface Window { OpenDesktop: { Logo: typeof Logo; Button: typeof Button; Status: typeof Status; StageTable: typeof StageTable; VMRow: typeof VMRow } }
}
