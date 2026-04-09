/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        // Sourcetree-inspired palette
        surface: {
          DEFAULT: '#ffffff',
          secondary: '#f6f8fa',
          sidebar: '#f0f1f3',
          hover: '#e8eaed',
          active: '#dfe1e5',
          border: '#d1d5da',
        },
        ink: {
          DEFAULT: '#24292f',
          secondary: '#57606a',
          muted: '#8b949e',
          faint: '#b1bac4',
        },
        // Sourcetree blue
        st: {
          blue: '#0052cc',
          'blue-light': '#deebff',
          'blue-hover': '#0065ff',
        },
        status: {
          green: '#1a7f37',
          'green-bg': '#dafbe1',
          yellow: '#9a6700',
          'yellow-bg': '#fff8c5',
          red: '#cf222e',
          'red-bg': '#ffebe9',
        },
      },
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', 'Segoe UI', 'Noto Sans', 'Helvetica', 'Arial', 'sans-serif'],
        mono: ['SF Mono', 'Menlo', 'Consolas', 'Liberation Mono', 'monospace'],
      },
      fontSize: {
        '2xs': ['0.6875rem', '1rem'],
      },
      boxShadow: {
        'toolbar': '0 1px 0 0 #d1d5da',
        'sidebar': '1px 0 0 0 #d1d5da',
        'panel': '0 1px 3px rgba(0,0,0,0.08)',
      },
    },
  },
  plugins: [],
}
