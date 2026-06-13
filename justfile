# golf-booker dev tasks

# Regenerate CSS from design tokens, then bundle into assets/styles.css
css:
  npm run css

# Watch tokens and regenerate variables on change (run alongside `dev-web`)
css-watch:
  npm run css:watch

# Run the server, restarting on source changes
dev-web:
  cargo watch -x run

# Run CSS watch + web server together
dev:
  #!/bin/sh
  just css-watch &
  pid1=$!
  just dev-web &
  pid2=$!
  trap "kill $pid1 $pid2" EXIT
  wait $pid1 $pid2

# Type-check
check:
  cargo check

# Run tests
test:
  cargo test
