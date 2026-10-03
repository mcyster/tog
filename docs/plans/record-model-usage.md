# Record portable usage on each model response

Extend the Usage mixin on `ModelResponse` so readers can understand token usage
and reported cost without knowing which driver ran the call. Every attribute is
optional. Each driver supplies the values it can provide and translates them into
the shared meanings below.

Mark accepted this direction on 2026-10-03. This is an implementation plan.
Today, `ModelResponse` already holds an optional `Usage`, but that value requires
both input and output counts. The OpenAI driver records usage only when both
counts are present. The additional attributes and independent optionality remain
to be implemented.

## Boundary and behavior

Usage belongs to one model invocation, on its `model_response`. Each retry has
its own response and usage. Conversation totals and reports derive from those
records; they must not count turn summaries as additional consumption.

Conversation owns this contract. Drivers normalize provider reports at the
boundary, including providers whose cache or reasoning counts are reported
separately from their totals. Readers should not need provider-specific arithmetic.
This follows the [conversation boundary](../architecture/conversation.md): shared
meaning belongs to the conversation, and provider translation belongs to drivers.

Absence means unknown. Zero means a known zero. A driver can report cost without
token counts, or one count without the others. Unsupported attributes remain
absent; no driver must invent values to fill the contract.

Failure does not imply zero consumption. Preserve reported usage when an attempt
fails, and when cancellation is supported, when it is cancelled. Stream updates
must resolve to one invocation's usage rather than being added repeatedly.

`cost_usd` is the provider's reported cost. Price-table estimates are outside
this contract and must not be silently recorded as reported cost. Do not add
`source` or `completeness` attributes initially: the source has a fixed meaning,
and individual optional values express missing information. Recording estimates
or incomplete stream counters would require a separate explicit contract.

## Attribute contract

Keep `Usage` composed into `ModelResponse` through its existing `usage` field.
The term mixin describes this shared group of attributes, not an inheritance
hierarchy. Every attribute below is optional.

| Attribute | Meaning |
| --- | --- |
| `input_tokens` | All input tokens, including cache reads and writes |
| `output_tokens` | All generated tokens, including reasoning |
| `total_tokens` | Input plus output tokens |
| `cache_read_input_tokens` | Input tokens read from cache |
| `cache_write_input_tokens` | Input tokens written to cache |
| `reasoning_output_tokens` | Output tokens spent on reasoning |
| `cost_usd` | Provider-reported cost of this invocation, in US dollars |

Token counts are nonnegative whole numbers. Cost is a nonnegative monetary value
in dollars, including fractions of a dollar; choose and document its concrete
precision and serialization when implementing it.

Cache reads and writes are disjoint subsets of input. Reasoning is a subset of
output. Where the relevant values are known, their relationships must agree:
cache reads plus writes cannot exceed input, reasoning cannot exceed output, and
total equals input plus output. An absent subset does not mean zero.

Two derived values need no stored attributes:

```text
uncached_input_tokens = input_tokens
                     - cache_read_input_tokens
                     - cache_write_input_tokens

non_reasoning_output_tokens = output_tokens
                            - reasoning_output_tokens
```

A derived value is known only when all of its required components are known.
Drivers may derive totals from known components under the same rule.

## Implementation and completion

Extend the existing Usage contract and its immutable accessors, then update the
OpenAI mapping to preserve each independently available value. Keep the shared
contract independent of provider implementations. An inconsistent provider report
needs an explicit handling policy during implementation; it must not silently
become a valid but misleading usage record.

Verify absent versus zero values, independently reported attributes, subset
relationships, serialization, and the driver's mapping with concrete provider
fixtures. Verify that downstream totals do not double-count cache or reasoning
subsets or retries. No live paid model calls are needed for these checks.

This plan is complete when model responses preserve supported reported values
under this contract and readers can consume them without provider-specific
knowledge. Usage display, price estimation, budgets, automatic retries, and new
drivers are separate work.
