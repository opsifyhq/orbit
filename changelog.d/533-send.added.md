- Exchange EWS `mail.send`, `mail.reply` and `mail.forward` capabilities. Replies use the
  original sender/subject; forwards carry attachments. Recipient allowlists support exact
  addresses, `*@domain` and explicit `*`; sending remains an external side effect and
  requires a send binding.
