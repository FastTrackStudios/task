import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

// JSON.stringify that survives bigint fields (i64/u64 land as `bigint`).
export function dump(value: unknown, indent = 2): string {
  return JSON.stringify(
    value,
    (_key, v) => (typeof v === "bigint" ? v.toString() : v),
    indent,
  )
}
