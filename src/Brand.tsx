export function NavIcon({ kind }: { kind: 'studio' | 'equalizer' | 'settings' | 'eyes' }) {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {kind === 'eyes' ? <><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></> : kind === 'studio' ? <><rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/></> : kind === 'equalizer' ? <><path d="M5 3v18M12 3v18M19 3v18"/><rect x="3" y="7" width="4" height="4" rx="1" fill="var(--bg)"/><rect x="10" y="14" width="4" height="4" rx="1" fill="var(--bg)"/><rect x="17" y="5" width="4" height="4" rx="1" fill="var(--bg)"/></> : <><circle cx="12" cy="12" r="3"/><path d="m9 3 .6 2.4-2 .9-2.2-1.1-2.2 3.8 2 1.4v2.3l-2 1.4 2.2 3.8 2.2-1.1 2 .9L10 21h4l.6-2.4 2-.9 2.2 1.1 2.2-3.8-2-1.4v-2.3l2-1.4-2.2-3.8-2.2 1.1-2-.9L14 3Z"/></>}
  </svg>;
}
export function WindowGlyph({ kind }: { kind: string }) {
  return <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.1" aria-hidden="true">{kind==='minimize'?<path d="M0 5h10"/>:kind==='maximize'?<rect x=".5" y=".5" width="9" height="9"/>:<path d="m1 1 8 8M9 1 1 9"/>}</svg>;
}
