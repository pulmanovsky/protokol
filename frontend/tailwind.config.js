/** @type {import('tailwindcss').Config} */
module.exports = {
    darkMode: ['class'],
    content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
  	extend: {
  		fontFamily: {
  			sans: [
  				'-apple-system',
  				'BlinkMacSystemFont',
  				'"SF Pro Text"',
  				'"SF Pro Display"',
  				'"Helvetica Neue"',
  				'system-ui',
  				'sans-serif'
  			]
  		},
  		colors: {
  			// Neutral ramp and graphite accent come from CSS variables so light/dark swap in one place
  			gray: {
  				'50': 'rgb(var(--gray-50) / <alpha-value>)',
  				'100': 'rgb(var(--gray-100) / <alpha-value>)',
  				'200': 'rgb(var(--gray-200) / <alpha-value>)',
  				'300': 'rgb(var(--gray-300) / <alpha-value>)',
  				'400': 'rgb(var(--gray-400) / <alpha-value>)',
  				'500': 'rgb(var(--gray-500) / <alpha-value>)',
  				'600': 'rgb(var(--gray-600) / <alpha-value>)',
  				'700': 'rgb(var(--gray-700) / <alpha-value>)',
  				'800': 'rgb(var(--gray-800) / <alpha-value>)',
  				'900': 'rgb(var(--gray-900) / <alpha-value>)',
  				'950': 'rgb(var(--gray-950) / <alpha-value>)',
  			},
  			slate: {
  				'50': 'rgb(var(--gray-50) / <alpha-value>)',
  				'100': 'rgb(var(--gray-100) / <alpha-value>)',
  				'200': 'rgb(var(--gray-200) / <alpha-value>)',
  				'300': 'rgb(var(--gray-300) / <alpha-value>)',
  				'400': 'rgb(var(--gray-400) / <alpha-value>)',
  				'500': 'rgb(var(--gray-500) / <alpha-value>)',
  				'600': 'rgb(var(--gray-600) / <alpha-value>)',
  				'700': 'rgb(var(--gray-700) / <alpha-value>)',
  				'800': 'rgb(var(--gray-800) / <alpha-value>)',
  				'900': 'rgb(var(--gray-900) / <alpha-value>)',
  				'950': 'rgb(var(--gray-950) / <alpha-value>)',
  			},
  			zinc: {
  				'50': 'rgb(var(--gray-50) / <alpha-value>)',
  				'100': 'rgb(var(--gray-100) / <alpha-value>)',
  				'200': 'rgb(var(--gray-200) / <alpha-value>)',
  				'300': 'rgb(var(--gray-300) / <alpha-value>)',
  				'400': 'rgb(var(--gray-400) / <alpha-value>)',
  				'500': 'rgb(var(--gray-500) / <alpha-value>)',
  				'600': 'rgb(var(--gray-600) / <alpha-value>)',
  				'700': 'rgb(var(--gray-700) / <alpha-value>)',
  				'800': 'rgb(var(--gray-800) / <alpha-value>)',
  				'900': 'rgb(var(--gray-900) / <alpha-value>)',
  				'950': 'rgb(var(--gray-950) / <alpha-value>)',
  			},
  			neutral: {
  				'50': 'rgb(var(--gray-50) / <alpha-value>)',
  				'100': 'rgb(var(--gray-100) / <alpha-value>)',
  				'200': 'rgb(var(--gray-200) / <alpha-value>)',
  				'300': 'rgb(var(--gray-300) / <alpha-value>)',
  				'400': 'rgb(var(--gray-400) / <alpha-value>)',
  				'500': 'rgb(var(--gray-500) / <alpha-value>)',
  				'600': 'rgb(var(--gray-600) / <alpha-value>)',
  				'700': 'rgb(var(--gray-700) / <alpha-value>)',
  				'800': 'rgb(var(--gray-800) / <alpha-value>)',
  				'900': 'rgb(var(--gray-900) / <alpha-value>)',
  				'950': 'rgb(var(--gray-950) / <alpha-value>)',
  			},
  			blue: {
  				'50': 'rgb(var(--accent-50) / <alpha-value>)',
  				'100': 'rgb(var(--accent-100) / <alpha-value>)',
  				'200': 'rgb(var(--accent-200) / <alpha-value>)',
  				'300': 'rgb(var(--accent-300) / <alpha-value>)',
  				'400': 'rgb(var(--accent-400) / <alpha-value>)',
  				'500': 'rgb(var(--accent-500) / <alpha-value>)',
  				'600': 'rgb(var(--accent-600) / <alpha-value>)',
  				'700': 'rgb(var(--accent-700) / <alpha-value>)',
  				'800': 'rgb(var(--accent-800) / <alpha-value>)',
  				'900': 'rgb(var(--accent-900) / <alpha-value>)',
  				'950': 'rgb(var(--accent-950) / <alpha-value>)',
  			},
  			red: {
  				'500': '#D93A30',
  				'600': '#C2342B'
  			},
  			background: 'hsl(var(--background))',
  			foreground: 'hsl(var(--foreground))',
  			border: 'hsl(var(--border))',
  			input: 'hsl(var(--input))',
  			ring: 'hsl(var(--ring))',
  			primary: {
  				DEFAULT: 'hsl(var(--primary))',
  				foreground: 'hsl(var(--primary-foreground))'
  			},
  			secondary: {
  				DEFAULT: 'hsl(var(--secondary))',
  				foreground: 'hsl(var(--secondary-foreground))'
  			},
  			tertiary: '#64748b',
  			card: {
  				DEFAULT: 'hsl(var(--card))',
  				foreground: 'hsl(var(--card-foreground))'
  			},
  			popover: {
  				DEFAULT: 'hsl(var(--popover))',
  				foreground: 'hsl(var(--popover-foreground))'
  			},
  			muted: {
  				DEFAULT: 'hsl(var(--muted))',
  				foreground: 'hsl(var(--muted-foreground))'
  			},
  			accent: {
  				DEFAULT: 'hsl(var(--accent))',
  				foreground: 'hsl(var(--accent-foreground))'
  			},
  			destructive: {
  				DEFAULT: 'hsl(var(--destructive))',
  				foreground: 'hsl(var(--destructive-foreground))'
  			},
  			chart: {
  				'1': 'hsl(var(--chart-1))',
  				'2': 'hsl(var(--chart-2))',
  				'3': 'hsl(var(--chart-3))',
  				'4': 'hsl(var(--chart-4))',
  				'5': 'hsl(var(--chart-5))'
  			}
  		},
  		borderRadius: {
  			lg: 'var(--radius)',
  			md: 'calc(var(--radius) - 2px)',
  			sm: 'calc(var(--radius) - 4px)'
  		},
  		keyframes: {
  			'accordion-down': {
  				from: {
  					height: '0'
  				},
  				to: {
  					height: 'var(--radix-accordion-content-height)'
  				}
  			},
  			'accordion-up': {
  				from: {
  					height: 'var(--radix-accordion-content-height)'
  				},
  				to: {
  					height: '0'
  				}
  			}
  		},
  		animation: {
  			'accordion-down': 'accordion-down 0.2s ease-out',
  			'accordion-up': 'accordion-up 0.2s ease-out'
  		}
  	}
  },
  plugins: [
    require("tailwindcss-animate"),
    require("@tailwindcss/typography"),
    require("@tailwindcss/container-queries"),
  ],
}