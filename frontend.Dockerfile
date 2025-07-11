# Use Node.js for development
FROM node:20 AS builder

WORKDIR /app/frontend

# Copy package.json and lock file first to optimize caching
COPY frontend/package.json frontend/package-lock.json ./

# Install dependencies
RUN npm install

# Copy the rest of the project files
COPY frontend .

# Build for production
RUN npm run build

# Install static server
RUN npm install -g serve

# Expose the Vite development server port (default is 5173)
EXPOSE 5173

# Start the Vite development server
CMD ["serve", "-s", "dist", "-l", "5173"]
