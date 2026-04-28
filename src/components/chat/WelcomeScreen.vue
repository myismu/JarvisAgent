<template>
  <div class="welcome-screen">
    <div class="arc-reactor-container">
      <svg class="arc-reactor" viewBox="0 0 400 400" xmlns="http://www.w3.org/2000/svg">
        <defs>
          <radialGradient id="coreGlowW" cx="50%" cy="50%" r="50%">
            <stop offset="0%" stop-color="#ffffff" stop-opacity="1" />
            <stop offset="20%" stop-color="#60a5fa" stop-opacity="0.9" />
            <stop offset="50%" stop-color="#3b82f6" stop-opacity="0.4" />
            <stop offset="100%" stop-color="#1e3a5f" stop-opacity="0" />
          </radialGradient>
          <radialGradient id="innerGlowW" cx="50%" cy="50%" r="50%">
            <stop offset="0%" stop-color="#93c5fd" stop-opacity="0.8" />
            <stop offset="60%" stop-color="#3b82f6" stop-opacity="0.3" />
            <stop offset="100%" stop-color="#1e40af" stop-opacity="0" />
          </radialGradient>
        </defs>

        <circle cx="200" cy="200" r="195" fill="none" stroke="#1e3a5f" stroke-width="0.5" opacity="0.3" />
        <circle cx="200" cy="200" r="185" fill="none" stroke="#2563eb" stroke-width="0.8" opacity="0.2" />

        <g class="ring-outer">
          <circle cx="200" cy="200" r="170" fill="none" stroke="#3b82f6" stroke-width="1.5" opacity="0.6" />
          <circle cx="200" cy="200" r="165" fill="none" stroke="#60a5fa" stroke-width="0.5" opacity="0.3" stroke-dasharray="8 12" />
        </g>

        <g class="ring-segments">
          <circle cx="200" cy="200" r="155" fill="none" stroke="#2563eb" stroke-width="2" opacity="0.5"
            stroke-dasharray="20 15 10 15 20 15 10 15 20 15 10 15 20 15 10 15 20 15 10 15 20 15 10 15" />
        </g>

        <g class="ring-middle">
          <circle cx="200" cy="200" r="140" fill="none" stroke="#3b82f6" stroke-width="1" opacity="0.4" />
          <circle cx="200" cy="200" r="135" fill="none" stroke="#60a5fa" stroke-width="0.5" opacity="0.25" stroke-dasharray="4 8" />
        </g>

        <g class="triangles">
          <polygon points="200,55 230,100 170,100" fill="none" stroke="#3b82f6" stroke-width="1.2" opacity="0.6" />
          <polygon points="345,200 300,230 300,170" fill="none" stroke="#3b82f6" stroke-width="1.2" opacity="0.6" />
          <polygon points="200,345 170,300 230,300" fill="none" stroke="#3b82f6" stroke-width="1.2" opacity="0.6" />
          <polygon points="55,200 100,170 100,230" fill="none" stroke="#3b82f6" stroke-width="1.2" opacity="0.6" />
          <polygon points="295,80 305,125 265,100" fill="none" stroke="#2563eb" stroke-width="0.8" opacity="0.4" />
          <polygon points="320,295 275,305 300,265" fill="none" stroke="#2563eb" stroke-width="0.8" opacity="0.4" />
          <polygon points="105,320 95,275 135,300" fill="none" stroke="#2563eb" stroke-width="0.8" opacity="0.4" />
          <polygon points="80,105 125,95 100,135" fill="none" stroke="#2563eb" stroke-width="0.8" opacity="0.4" />
        </g>

        <g class="ring-inner-outer">
          <circle cx="200" cy="200" r="110" fill="none" stroke="#3b82f6" stroke-width="1.5" opacity="0.5" />
        </g>

        <g class="hex-ring">
          <polygon points="200,95 290,147 290,253 200,305 110,253 110,147" fill="none" stroke="#60a5fa" stroke-width="1" opacity="0.35" />
        </g>

        <g class="ring-inner">
          <circle cx="200" cy="200" r="85" fill="none" stroke="#3b82f6" stroke-width="2" opacity="0.6" />
          <circle cx="200" cy="200" r="80" fill="none" stroke="#60a5fa" stroke-width="0.5" opacity="0.3" stroke-dasharray="6 10" />
        </g>

        <g class="beam-lines">
          <line x1="200" y1="115" x2="200" y2="80" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
          <line x1="272" y1="155" x2="300" y2="135" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
          <line x1="272" y1="245" x2="300" y2="265" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
          <line x1="200" y1="285" x2="200" y2="320" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
          <line x1="128" y1="245" x2="100" y2="265" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
          <line x1="128" y1="155" x2="100" y2="135" stroke="#60a5fa" stroke-width="1" opacity="0.4" />
        </g>

        <g class="ring-core-outer">
          <circle cx="200" cy="200" r="60" fill="none" stroke="#3b82f6" stroke-width="2.5" opacity="0.7" />
        </g>

        <g class="ring-core-segments">
          <path d="M 200 145 A 55 55 0 0 1 248 172" fill="none" stroke="#60a5fa" stroke-width="2" opacity="0.5" />
          <path d="M 255 200 A 55 55 0 0 1 228 248" fill="none" stroke="#60a5fa" stroke-width="2" opacity="0.5" />
          <path d="M 200 255 A 55 55 0 0 1 152 228" fill="none" stroke="#60a5fa" stroke-width="2" opacity="0.5" />
          <path d="M 145 200 A 55 55 0 0 1 172 152" fill="none" stroke="#60a5fa" stroke-width="2" opacity="0.5" />
        </g>

        <g class="ring-core-inner">
          <circle cx="200" cy="200" r="40" fill="none" stroke="#60a5fa" stroke-width="1.5" opacity="0.5" />
        </g>

        <g class="core-pulse">
          <circle cx="200" cy="200" r="28" fill="url(#innerGlowW)" />
          <circle cx="200" cy="200" r="18" fill="url(#coreGlowW)" />
          <circle cx="200" cy="200" r="8" fill="#ffffff" opacity="0.9" />
        </g>

        <g class="hud-data" opacity="0.3">
          <text x="200" y="42" text-anchor="middle" fill="#60a5fa" font-family="monospace" font-size="8">SYS.ONLINE</text>
          <text x="358" y="204" text-anchor="middle" fill="#60a5fa" font-family="monospace" font-size="8">PWR.100%</text>
          <text x="200" y="368" text-anchor="middle" fill="#60a5fa" font-family="monospace" font-size="8">NET.LINK</text>
          <text x="42" y="204" text-anchor="middle" fill="#60a5fa" font-family="monospace" font-size="8">SEC.OK</text>
        </g>

        <g class="scan-line" opacity="0.15">
          <line x1="200" y1="200" x2="200" y2="5" stroke="#60a5fa" stroke-width="0.5" />
        </g>
      </svg>

      <div class="reactor-label">
        <span class="label-char" style="--i:0">J</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:1">A</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:2">R</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:3">V</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:4">I</span><span class="label-dot">.</span>
        <span class="label-char" style="--i:5">S</span>
      </div>
    </div>
    <div class="welcome-text">随时准备为您效劳，先生。</div>
  </div>
</template>

<style scoped>
.welcome-screen {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 400px;
  gap: var(--space-lg);
  pointer-events: none;
  user-select: none;
}

.arc-reactor-container {
  position: relative;
  width: 240px;
  height: 240px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.arc-reactor {
  width: 100%;
  height: 100%;
  filter: drop-shadow(0 0 20px rgba(59, 130, 246, 0.3));
  animation: reactorPulse 4s ease-in-out infinite;
}

.reactor-label {
  position: absolute;
  bottom: -10px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  font-size: 16px;
  font-weight: 600;
  letter-spacing: 2px;
  color: var(--accent-blue);
}

.label-char {
  animation: labelGlow 3s ease-in-out infinite;
  animation-delay: calc(var(--i) * 0.15s);
}

.label-dot {
  opacity: 0.5;
  animation: dotPulse 2s ease-in-out infinite;
  animation-delay: calc(var(--i, 0) * 0.15s + 0.7s);
}

.welcome-text {
  color: var(--text-muted);
  font-size: 14px;
  letter-spacing: 1px;
}

@keyframes reactorPulse {
  0%, 100% { filter: drop-shadow(0 0 20px rgba(59, 130, 246, 0.3)); }
  50% { filter: drop-shadow(0 0 35px rgba(59, 130, 246, 0.5)); }
}

@keyframes labelGlow {
  0%, 100% { opacity: 0.6; }
  50% { opacity: 1; }
}

@keyframes dotPulse {
  0%, 100% { opacity: 0.3; }
  50% { opacity: 0.9; }
}

.ring-outer, .ring-segments, .ring-middle, .ring-inner, .ring-core-outer, .core-pulse {
  transform-origin: 200px 200px;
  animation: ringRotateCW 20s linear infinite;
}

.ring-segments, .ring-middle, .ring-inner-outer, .hex-ring {
  animation: ringRotateCCW 25s linear infinite;
}

.triangles {
  transform-origin: 200px 200px;
  animation: ringRotateCW 30s linear infinite;
}

.beam-lines {
  transform-origin: 200px 200px;
  animation: ringRotateCCW 15s linear infinite;
}

.ring-core-segments, .ring-core-inner {
  transform-origin: 200px 200px;
  animation: ringRotateCW 8s linear infinite;
}

.scan-line {
  transform-origin: 200px 200px;
  animation: scanSpin 12s linear infinite;
}

@keyframes ringRotateCW {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

@keyframes ringRotateCCW {
  from { transform: rotate(360deg); }
  to { transform: rotate(0deg); }
}

@keyframes scanSpin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
</style>
