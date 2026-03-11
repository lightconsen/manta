# Calculator Skill

Perform mathematical calculations and conversions.

## Triggers

- Regex: `calculate\s+(.+)`
- Regex: `=\s*(.+)`
- Regex: `([\d\s+\-*/%^().]+)=?`
- Keyword: "calculate"
- Intent: "calculate"

## Prompt

When the user asks for calculations, use the `execute_code` tool with Python to compute the result.

Always show:
- The original expression
- The computed result
- Any relevant unit conversions

## Example Usage

**User:** "Calculate 15 * 23 + 7"

**Action:** Execute Python code to compute the expression

**Response:**
```
Calculation: 15 * 23 + 7
Result: 352
```

**User:** "Convert 100 USD to EUR"

**Action:** Use web search to find current exchange rate, then calculate

**Response:**
```
100 USD = 92.50 EUR
(Exchange rate: 1 USD = 0.925 EUR)
```

**User:** "What's the square root of 144?"

**Action:** Calculate using Python

**Response:**
```
√144 = 12
```

## Supported Operations

- Basic arithmetic: +, -, *, /, %
- Powers: **, ^
- Roots: sqrt(), cbrt()
- Trigonometry: sin(), cos(), tan()
- Logarithms: log(), log10(), ln()
- Constants: pi, e
- Conversions: currency, units

## Security

- All calculations run in a sandboxed Python environment
- Network access is restricted
- Execution time is limited to 5 seconds
