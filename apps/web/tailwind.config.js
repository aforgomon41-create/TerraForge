/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{vue,js,ts}'],
  theme: {
    extend: {
      colors: {
        bg: '#0A0C10',
        panel: 'rgba(18,21,28,0.72)',
        line: 'rgba(255,255,255,0.08)',
        ink: '#E8EAED',
        dim: '#9AA0A6',
        accent: '#C9A227',
        cool: '#4A9EFF',
        danger: '#E5484D',
        ok: '#30A46C',
      },
      fontFamily: {
        sans: ['Inter', 'Noto Sans SC', 'system-ui', 'sans-serif'],
        serif: ['Noto Serif SC', 'Georgia', 'serif'],
        mono: ['JetBrains Mono', 'Consolas', 'monospace'],
      },
      backdropBlur: {
        panel: '24px',
      },
    },
  },
  plugins: [],
}
