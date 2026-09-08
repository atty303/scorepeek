#!/usr/bin/awk -f

# Prefix ordinary selectors while leaving at-rules and keyframe steps intact.
# Skin CSS shares a document with the native editor, so its cascade must not
# escape the host-owned root. The OBS iframe uses the same root class.

function trim(value) {
  sub(/^[[:space:]]+/, "", value)
  sub(/[[:space:]]+$/, "", value)
  return value
}

function scoped(selector, position, char, quote, escaped, parentheses, brackets, part, result) {
  part = ""
  result = ""
  for (position = 1; position <= length(selector); position++) {
    char = substr(selector, position, 1)
    if (quote != "") {
      part = part char
      if (escaped) {
        escaped = 0
      } else if (char == "\\") {
        escaped = 1
      } else if (char == quote) {
        quote = ""
      }
    } else if (char == "\"" || char == "'") {
      quote = char
      part = part char
    } else if (char == "(") {
      parentheses++
      part = part char
    } else if (char == ")") {
      parentheses--
      part = part char
    } else if (char == "[") {
      brackets++
      part = part char
    } else if (char == "]") {
      brackets--
      part = part char
    } else if (char == "," && parentheses == 0 && brackets == 0) {
      result = result (result == "" ? "" : ",") ".scorepeek-skin-scope " trim(part)
      part = ""
    } else {
      part = part char
    }
  }
  return result (result == "" ? "" : ",") ".scorepeek-skin-scope " trim(part)
}

BEGIN {
  depth = 0
  context[0] = "sheet"
  pending = ""
  quote = ""
  escaped = 0
  parentheses = 0
  brackets = 0
  comment = 0
}

{
  source = source $0 "\n"
}

END {
  for (position = 1; position <= length(source); position++) {
    char = substr(source, position, 1)
    next_char = substr(source, position + 1, 1)
    current = context[depth]

    if (current == "rule") {
      if (comment) {
        printf "%s", char
        if (char == "*" && next_char == "/") {
          printf "%s", next_char
          position++
          comment = 0
        }
      } else if (quote != "") {
        printf "%s", char
        if (escaped) {
          escaped = 0
        } else if (char == "\\") {
          escaped = 1
        } else if (char == quote) {
          quote = ""
        }
      } else if (char == "/" && next_char == "*") {
        printf "%s%s", char, next_char
        position++
        comment = 1
      } else if (char == "\"" || char == "'") {
        printf "%s", char
        quote = char
      } else {
        printf "%s", char
        if (char == "}") {
          depth--
        }
      }
      continue
    }

    if (comment) {
      pending = pending char
      if (char == "*" && next_char == "/") {
        pending = pending next_char
        position++
        comment = 0
      }
      continue
    }
    if (quote != "") {
      pending = pending char
      if (escaped) {
        escaped = 0
      } else if (char == "\\") {
        escaped = 1
      } else if (char == quote) {
        quote = ""
      }
      continue
    }
    if (char == "/" && next_char == "*") {
      pending = pending char next_char
      position++
      comment = 1
      continue
    }
    if (char == "\"" || char == "'") {
      quote = char
      pending = pending char
      continue
    }

    if (char == "(") {
      parentheses++
      pending = pending char
      continue
    }
    if (char == ")") {
      parentheses--
      pending = pending char
      continue
    }
    if (char == "[") {
      brackets++
      pending = pending char
      continue
    }
    if (char == "]") {
      brackets--
      pending = pending char
      continue
    }

    if (char == "{" && parentheses == 0 && brackets == 0) {
      prelude = trim(pending)
      pending = ""
      if (current == "keyframes") {
        printf "%s{", prelude
        context[++depth] = "rule"
      } else if (prelude ~ /^@(-[[:alnum:]]+-)?keyframes[[:space:]]/) {
        printf "%s{", prelude
        context[++depth] = "keyframes"
      } else if (prelude ~ /^@(media|supports|container|layer)([^[:alnum:]_-]|$)/) {
        printf "%s{", prelude
        context[++depth] = "sheet"
      } else if (prelude ~ /^@/) {
        printf "%s{", prelude
        context[++depth] = "rule"
      } else {
        printf "%s{", scoped(prelude)
        context[++depth] = "rule"
      }
    } else if (char == "}" && depth > 0) {
      printf "%s}", pending
      pending = ""
      depth--
    } else {
      pending = pending char
    }
  }
  printf "%s", pending
}
