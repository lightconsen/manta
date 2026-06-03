/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        primary: {
          50: "#FAF0FB",
          100: "#F3E0F6",
          200: "#E7BFF0",
          300: "#D694E3",
          400: "#C44ED0",
          500: "#B22AC2",
          600: "#9A1EA8",
          700: "#7A1684",
          800: "#5E1066",
          900: "#4A0A50",
        },
      },
    },
  },
  plugins: [],
};
