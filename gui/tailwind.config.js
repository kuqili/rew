/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        // === v4 Design System — macOS Native ===
        sys: {
          blue: '#007aff',
          'blue-hover': '#0066d6',
          'blue-subtle': 'rgba(0,122,255,0.06)',
          green: '#34c759',
          'green-bg': 'rgba(52,199,89,0.08)',
          amber: '#ff9f0a',
          'amber-bg': 'rgba(255,159,10,0.08)',
          red: '#ff3b30',
          'red-bg': 'rgba(255,59,48,0.06)',
        },
        tool: {
          claude: '#da7b3a',
          'claude-bg': 'rgba(218,123,58,0.08)',
          cursor: '#6366f1',
          'cursor-bg': 'rgba(99,102,241,0.08)',
        },
        bg: {
          window: '#ffffff',
          sidebar: 'rgba(244,244,246,0.82)',
          grouped: '#f5f5f7',
          hover: 'rgba(0,0,0,0.035)',
          active: 'rgba(0,0,0,0.06)',
        },
        border: {
          DEFAULT: 'rgba(0,0,0,0.09)',
          light: 'rgba(0,0,0,0.05)',
        },
        t: {
          1: '#1d1d1f',
          2: '#6e6e73',
          3: '#aeaeb2',
          4: '#c7c7cc',
        },
        // Diff (dark theme)
        diff: {
          bg: '#1c1c1e',
          gutter: '#2c2c2e',
          text: '#e5e5e7',
          'add-bg': 'rgba(52,199,89,0.12)',
          'add-text': '#6ee7b7',
          'del-bg': 'rgba(255,59,48,0.10)',
          'del-text': '#fca5a5',
          ln: '#48484a',
        },
      },
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', 'SF Pro Text', 'Helvetica Neue', 'sans-serif'],
        mono: ['SF Mono', 'Menlo', 'Consolas', 'Liberation Mono', 'monospace'],
      },
      fontSize: {
        '2xs': ['0.6875rem', '1rem'],
      },
      animation: {
        'breathe': 'breathe 3s ease-in-out infinite',
      },
      keyframes: {
        breathe: {
          '0%, 100%': { opacity: '1', transform: 'scale(1)' },
          '50%': { opacity: '0.5', transform: 'scale(0.85)' },
        },
      },
    },
  },
  plugins: [],
}
