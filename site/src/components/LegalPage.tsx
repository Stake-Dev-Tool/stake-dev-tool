import type { ReactNode } from 'react'
import SectionRule from './SectionRule'

export default function LegalPage({
  title,
  updated,
  children,
}: {
  title: string
  updated: string
  children: ReactNode
}) {
  return (
    <main className="wrap pt-16 pb-8">
      <SectionRule label="legal" />
      <h1 className="display mt-12 mb-0 max-w-2xl text-4xl font-bold">{title}</h1>
      <p className="mt-4 mb-0 font-mono text-[0.7rem] tracking-[0.08em] text-faint">
        Last updated: {updated}
      </p>
      <div className="legal mt-6 max-w-2xl">{children}</div>
    </main>
  )
}
