# Share Inflation Defense Specification (Issues #860 & #861)

## Overview
DeFi vaults that follow the ERC-4626 standard conversion model can be susceptible to the "first depositor inflation attack":
1. Attacker makes an initial deposit of 1 stroop to mint 1 share.
2. Attacker transfers (donates) a large quantity (e.g., $10,000 \times 10^7$ stroops) directly to the vault or inflates assets.
3. Subsequent depositors depositing less than the inflated share price receive 0 shares due to integer division truncation, while the attacker redeems the entire pool.

## Implemented Mitigations

### 1. Virtual Shares and Assets Offset (Issue #860)
The vault introduces standard virtual offsets into conversion calculations:
$$\text{effective\_shares} = \text{total\_shares} + \text{VIRTUAL\_SHARES\_OFFSET}$$
$$\text{effective\_assets} = \text{total\_deposited} + \text{VIRTUAL\_ASSETS\_OFFSET}$$

Where:
- $\text{VIRTUAL\_SHARES\_OFFSET} = 1,000,000$ ($10^6$ units)
- $\text{VIRTUAL\_ASSETS\_OFFSET} = 1$ unit

#### Conversion Formulas:
$$\text{shares\_out} = \frac{\text{assets\_in} \times (\text{total\_shares} + 10^6)}{\text{total\_assets} + 1}$$
$$\text{assets\_out} = \frac{\text{shares\_in} \times (\text{total\_assets} + 1)}{\text{total\_shares} + 10^6}$$

#### Loss Analysis
- For normal depositors: The offset introduces at most 1 stroop of rounding loss across normal deposit and withdrawal sizes.
- For an attacker: To shift the share price significantly, an attacker would need to donate exponentially more assets ($>10^6 \times$ normal scale), with $>99.99\%$ of the donation permanently captured by the virtual offset.

### 2. Dead Share Burning on Initial Deposit (Issue #861)
On the very first deposit to an uninitialized vault:
- The deposit must mint at least $\text{MIN\_LIQUIDITY\_SHARES} + 1 = 1,001$ shares; otherwise, it reverts with `InsufficientInitialDeposit`.
- Exactly $\text{MIN\_LIQUIDITY\_SHARES} = 1,000$ shares are permanently minted to dead storage and excluded from all distributions and redemption.
- The total share supply can never reach zero again, preventing the pool from ever returning to an unseeded state.
