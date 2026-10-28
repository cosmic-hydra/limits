# Greet the user named by $GREET_NAME (or "stranger"), in the language from $GREET_LANG.
import os

name = os.environ.get("GREET_NAME", "")
lang = os.environ.get("GREET_LANG", "")
greetings = {"en": "Hello", "fr": "Bonjour", "es": "Hola", "de": "Hallo"}
word = greetings.get(lang, "Hello")
if lang != "" and lang not in greetings:
    print("unknown language", lang + ", using en")
print(word + ", " + (name if name != "" else "stranger") + "!")
