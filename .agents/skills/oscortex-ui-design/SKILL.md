---
name: oscortex-ui-design
description: Design systems guidelines, layout rules, and components code-base for OSCortex landing page and interfaces, derived from lightfield.app style.
---

# OSCortex Design System Guideline (Lightfield Light Theme Style)

This document establishes the UI design language for the OSCortex landing page and interface modules. Every component must strictly align with these parameters to maintain visual continuity.

---

## 1. Design Tokens & Styling Variables

### Theme Colors (Light Mode)
* **Primary Background (Base):** `#f5f5f5` (Creamy off-white background)
* **Secondary Background:** `#fafafa`
* **Surface Card / secondary:** `rgba(0, 0, 0, 0.02)` (Very low opacity black overlay)
* **Surface Card / primary:** `rgba(0, 0, 0, 0.04)`
* **Hairline Border:** `rgba(0, 0, 0, 0.08)` (Ultra-thin borders)
* **Strong Border:** `rgba(0, 0, 0, 0.15)`
* **Text Primary:** `#0d0d0d` (High-contrast near-black text)
* **Text Muted:** `rgba(0, 0, 0, 0.65)` (Standard gray text)
* **Text Dim:** `rgba(0, 0, 0, 0.4)` (Light gray for tags/meta)
* **Accent Color:** `#7c3aed` (Violet) / `#10b981` (Emerald green for active state)
* **Buttons:**
  * **Primary:** solid black background with white text (`bg-black text-white hover:bg-neutral-800`)
  * **Secondary:** transparent background, light black border (`border-black/[0.08] text-black hover:bg-black/[0.02]`)

### Typography Hierarchy
* **Title font:** `Instrument Serif`, Georgia, serif (Normal/medium weight, tight tracking `-0.03em`)
* **Body font:** `Inter`, system-ui (Regular weight)
* **Monospace font:** `JetBrains Mono` (For telemetry, code snippets, tags)
* **Font Scales:**
  * `.text-d4`: `32px` (leading `1.15em`, tracking `-0.035em`)
  * `.text-h1`: `28px` (leading `1.2em`, tracking `-0.03em`)
  * `.text-h2`: `24px` (leading `1.25em`, tracking `-0.02em`)
  * `.text-h3`: `21px` (leading `1.25em`, tracking `-0.015em`)
  * `.text-h4`: `19px` (leading `1.3em`, tracking `-0.01em`)
  * `.text-base`: `15px`
  * `.text-sm`: `13px`
  * `.text-xs`: `12px`
  * `.text-xxs`: `11px`
  * `.monocaps-xxs`: `9px` (letter-spacing `1px`)
  * `.monocaps-xs`: `10px` (letter-spacing `1px`)

---

## 2. Structural Patterns

### Floating Capsule Navbar
* Navbar must render as a floating pill capsule centered at the top of the viewport.
* It must have a blur backdrop (`backdrop-blur-[17px]`), rounded corners (`rounded-[10px]`), and a hairline border.
* Example Tailwind class layout:
  ```html
  <nav class="fixed top-4 left-1/2 -translate-x-1/2 z-50 flex items-center gap-6 px-6 py-2 rounded-[10px] border border-black/[0.08] bg-white/80 backdrop-blur-md">
  ```

### Centered Hero Section
* **Big H1 Title:** Centered, broken gracefully into two lines using `<br />`, with the second line having a gradient:
  ```html
  <h1 class="text-center text-4xl font-semibold leading-[1.08] tracking-[-0.03em] text-black sm:text-6xl">
    The OS that builds itself <br />
    <span class="bg-gradient-to-r from-black via-zinc-700 to-black bg-clip-text text-transparent">around your hardware.</span>
  </h1>
  ```
* **Centered Signup Form / CTA Block:**
  * Clean email input field with a submit arrow button nested on the right.
  * Followed by a secondary "Get Started" or "Download SDK" button.
  * Capsule width restricted to `max-w-md` (centered).

---

## 3. Reusable UI Components

### Sleek Email Input Form (Lightfield Style)
```tsx
import React from 'react'

export function WaitlistForm() {
  return (
    <form className="relative w-full max-w-sm mx-auto">
      <input
        type="email"
        placeholder="Enter your work email..."
        className="h-10 w-full rounded-lg border border-black/[0.08] bg-white px-4 pr-10 text-black text-xs outline-none placeholder:text-neutral-400 focus:border-black/20 focus:ring-1 focus:ring-black/10 transition-all"
      />
      <button
        type="submit"
        aria-label="Submit"
        className="absolute top-1/2 right-3 -translate-y-1/2 text-neutral-500 hover:text-black transition-colors"
      >
        <svg className="w-4.5 h-4.5" fill="none" stroke="currentColor" strokeWidth="2" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" d="M13.5 4.5L21 12m0 0l-7.5 7.5M21 12H3" />
        </svg>
      </button>
    </form>
  )
}
```

### Capsule Pill Badge
```tsx
export function StatusPill({ label }: { label: string }) {
  return (
    <div className="inline-flex items-center gap-1.5 rounded-full border border-black/[0.06] bg-black/[0.02] px-2.5 py-0.5">
      <span className="h-1 w-1 rounded-full bg-emerald-500 anim-pulse-dot" />
      <span className="font-mono text-[9px] uppercase tracking-wider text-neutral-500 font-medium">
        {label}
      </span>
    </div>
  )
}
```
