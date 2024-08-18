import pandas as pd
import matplotlib.pyplot as plt

# Load the CSV file into a DataFrame
df = pd.read_csv('mm_state.csv')

# Display the first few rows of the DataFrame before removal
print("Before removal:")
print(df.head())

# Remove the first two rows
df = df.iloc[2:]

# Display the first few rows of the DataFrame after removal to confirm
print("After removal:")
print(df.head())

# Plot the columns: aevo_bid, aevo_ask, hl_bid, hl_ask
plt.figure(figsize=(10, 6))

# Plot aevo_bid and aevo_ask
plt.plot(df['aevo_bid'], label='Aevo Bid', linestyle='-', marker='o')
plt.plot(df['aevo_ask'], label='Aevo Ask', linestyle='-', marker='x')

# Plot hl_bid and hl_ask
plt.plot(df['hl_bid'], label='HL Bid', linestyle='-', marker='s')
plt.plot(df['hl_ask'], label='HL Ask', linestyle='-', marker='d')

# Add labels and title
plt.xlabel('Index')
plt.ylabel('Price')
plt.title('Aevo and Hyperliquid Bid/Ask Prices (After Removing First Two Rows)')

# Add a legend
plt.legend()

# Show the plot
plt.show()
