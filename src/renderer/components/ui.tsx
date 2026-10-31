import type { ButtonHTMLAttributes, ReactNode, InputHTMLAttributes } from 'react'
import './ui.css'

/* -------------------------------------------------------------- Button */

type Variant = 'primary' | 'live' | 'subtle' | 'ghost' | 'danger'
type Size = 'sm' | 'md' | 'lg'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
  icon?: ReactNode
  loading?: boolean
  block?: boolean
}

export function Button({
  variant = 'subtle',
  size = 'md',
  icon,
  loading = false,
  block = false,
  children,
  className = '',
  disabled,
  ...rest
}: ButtonProps): React.JSX.Element {
  return (
    <button
      className={`btn btn-${variant} btn-${size} ${block ? 'btn-block' : ''} ${className}`}
      disabled={disabled || loading}
      {...rest}
    >
      {loading ? <span className="spinner" /> : icon}
      {children != null && <span className="btn-label">{children}</span>}
    </button>
  )
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string
  active?: boolean
  variant?: 'plain' | 'danger'
}

export function IconButton({
  label,
  active = false,
  variant = 'plain',
  children,
  className = '',
  ...rest
}: IconButtonProps): React.JSX.Element {
  return (
    <button
      className={`icon-btn icon-btn-${variant} ${active ? 'is-active' : ''} ${className}`}
      title={label}
      aria-label={label}
      aria-pressed={active}
      {...rest}
    >
      {children}
    </button>
  )
}

/* ---------------------------------------------------------------- Card */

export function Card({
  children,
  className = '',
  ...rest
}: { children: ReactNode; className?: string } & React.HTMLAttributes<HTMLDivElement>): React.JSX.Element {
  return (
    <div className={`card ${className}`} {...rest}>
      {children}
    </div>
  )
}

/* --------------------------------------------------------------- Badge */

export function Badge({
  children,
  tone = 'neutral',
  icon
}: {
  children: ReactNode
  tone?: 'neutral' | 'accent' | 'live' | 'danger' | 'warning'
  icon?: ReactNode
}): React.JSX.Element {
  return (
    <span className={`badge badge-${tone}`}>
      {icon}
      {children}
    </span>
  )
}

/* -------------------------------------------------------------- Toggle */

export function Toggle({
  checked,
  onChange,
  label,
  hint,
  disabled = false
}: {
  checked: boolean
  onChange: (v: boolean) => void
  label: string
  hint?: string
  disabled?: boolean
}): React.JSX.Element {
  return (
    <label className={`toggle ${disabled ? 'is-disabled' : ''}`}>
      <span className="toggle-text">
        <span className="toggle-label">{label}</span>
        {hint && <span className="toggle-hint">{hint}</span>}
      </span>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="toggle-track" aria-hidden="true">
        <span className="toggle-thumb" />
      </span>
    </label>
  )
}

/* -------------------------------------------------------------- Slider */

interface SliderProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'onChange' | 'value'> {
  value: number
  min: number
  max: number
  step?: number
  onChange: (v: number) => void
}

export function Slider({ value, min, max, step = 1, onChange, ...rest }: SliderProps): React.JSX.Element {
  const pct = max > min ? Math.max(0, Math.min(100, ((value - min) / (max - min)) * 100)) : 0
  return (
    <input
      type="range"
      className="slider"
      value={value}
      min={min}
      max={max}
      step={step}
      onChange={(e) => onChange(Number(e.target.value))}
      style={{ ['--fill' as string]: `${pct}%` }}
      {...rest}
    />
  )
}

/* -------------------------------------------------------- Segmented */

export interface SegmentOption<T extends string> {
  value: T
  label: string
  hint?: string
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  size = 'md'
}: {
  options: SegmentOption<T>[]
  value: T
  onChange: (v: T) => void
  size?: 'sm' | 'md'
}): React.JSX.Element {
  return (
    <div className={`segmented segmented-${size}`} role="tablist">
      {options.map((o) => (
        <button
          key={o.value}
          role="tab"
          aria-selected={o.value === value}
          className={`segment ${o.value === value ? 'is-active' : ''}`}
          onClick={() => onChange(o.value)}
          title={o.hint}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

/* --------------------------------------------------------------- Field */

export function Field({
  label,
  hint,
  children,
  htmlFor
}: {
  label: string
  hint?: string
  children: ReactNode
  htmlFor?: string
}): React.JSX.Element {
  return (
    <div className="field">
      <div className="field-head">
        <label className="field-label" htmlFor={htmlFor}>
          {label}
        </label>
        {hint && <span className="field-hint">{hint}</span>}
      </div>
      {children}
    </div>
  )
}

/* ------------------------------------------------------------ TextInput */

export function TextInput(props: InputHTMLAttributes<HTMLInputElement>): React.JSX.Element {
  const { className = '', ...rest } = props
  return <input className={`text-input ${className}`} {...rest} />
}

/* ------------------------------------------------------------- StatPill */

export function StatPill({
  label,
  value,
  tone = 'neutral'
}: {
  label: string
  value: string
  tone?: 'neutral' | 'good' | 'warn' | 'bad'
}): React.JSX.Element {
  return (
    <div className={`stat-pill stat-${tone}`}>
      <span className="stat-value mono">{value}</span>
      <span className="stat-label">{label}</span>
    </div>
  )
}
