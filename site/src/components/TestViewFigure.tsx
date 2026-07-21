const EVENTS = [
  { tag: 'bet', body: 'mode=base   bet=1.00  win=0.00' },
  { tag: 'bet', body: 'mode=base   bet=1.00  win=2.40' },
  { tag: 'bet', body: 'mode=bonus  bet=1.00  win=142.30', win: true },
  { tag: 'mark', body: 'saved (bonus, #81423) · max round' },
  { tag: 'bet', body: 'mode=base   bet=1.00  win=12.50', win: true },
  { tag: 'replay', body: '(base, #2041) → win=25.00' },
  { tag: 'bet', body: 'mode=base   bet=1.00  win=0.00' },
  { tag: 'rtp', body: 'observed base=96.51%  bonus=96.48%' },
]

/**
 * The same mini slot front rendered in every viewport: three reels of
 * symbols with the middle payline lit up, a win readout and a spin button.
 * Identical content at every size is what sells "one game, N resolutions".
 */
function MiniGame() {
  return (
    <div className="flex h-full flex-col gap-[6%] p-[7%]">
      <div className="grid min-h-0 flex-1 grid-cols-3 grid-rows-3 gap-[4%]">
        {Array.from({ length: 9 }, (_, i) => {
          const onPayline = i >= 3 && i < 6
          return (
            <div
              key={i}
              className={`rounded-[0.2rem] ${
                onPayline ? 'border border-mint/40 bg-mint/15' : 'bg-line/70'
              }`}
            />
          )
        })}
      </div>
      <div className="flex shrink-0 items-center justify-between">
        <span className="font-mono text-[0.55rem] text-amber">142.30</span>
        <span className="h-[0.55rem] w-[0.55rem] rounded-full bg-mint/80" />
      </div>
    </div>
  )
}

function Viewport({
  label,
  ratio,
  width,
  delay,
  main,
}: {
  label: string
  ratio: string
  width: string
  delay: string
  main?: boolean
}) {
  return (
    <div className="frame-in flex min-w-0 flex-col" style={{ width, animationDelay: delay }}>
      <span className="mb-1.5 font-mono text-[0.58rem] tracking-[0.1em] whitespace-nowrap text-faint">
        {label}
      </span>
      <div
        className={`vframe w-full ${main ? 'vframe-main' : ''}`}
        style={{ aspectRatio: ratio }}
      >
        <MiniGame />
      </div>
    </div>
  )
}

export default function TestViewFigure() {
  return (
    <figure className="m-0">
      <div className="card rise overflow-hidden" style={{ animationDelay: '200ms' }}>
        {/* Window chrome */}
        <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
          <div className="flex items-center gap-2.5">
            <div className="flex gap-1.5" aria-hidden="true">
              <span className="h-2 w-2 rounded-full bg-line2" />
              <span className="h-2 w-2 rounded-full bg-line2" />
              <span className="h-2 w-2 rounded-full bg-line2" />
            </div>
            <span className="font-mono text-[0.66rem] tracking-[0.12em] text-moss uppercase">
              test view
            </span>
          </div>
          <div className="flex items-center gap-2">
            <span className="live-dot" aria-hidden="true" />
            <span className="font-mono text-[0.6rem] tracking-[0.1em] text-faint uppercase">
              live · SSE
            </span>
          </div>
        </div>

        {/* The same game, three resolutions, side by side */}
        <div className="flex items-end gap-3 px-4 pt-4 pb-5 sm:gap-4 sm:px-5" aria-hidden="true">
          <Viewport label="1920 × 1080" ratio="16 / 9" width="52%" delay="350ms" main />
          <Viewport label="1280 × 720" ratio="16 / 9" width="30%" delay="500ms" />
          <Viewport label="390 × 844" ratio="390 / 844" width="14%" delay="650ms" />
        </div>

        {/* Event stream, exactly like the app's last-event strip */}
        <div className="border-t border-line px-4 py-3 sm:px-5">
          <div className="ticker-mask h-[4.6rem]">
            <ul className="ticker-list">
              {[...EVENTS, ...EVENTS].map((event, i) => (
                <li
                  key={i}
                  className="flex gap-2.5 font-mono text-[0.64rem] leading-snug whitespace-nowrap sm:text-[0.7rem]"
                >
                  <span className="w-11 shrink-0 text-mint">{event.tag}</span>
                  <span className={event.win ? 'text-amber' : 'text-moss'}>{event.body}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </div>

      <figcaption className="mt-4 font-mono text-[0.68rem] tracking-[0.08em] text-faint">
        the multi-resolution test view · one session per frame
      </figcaption>
    </figure>
  )
}
