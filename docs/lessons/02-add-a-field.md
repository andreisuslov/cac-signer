# Add a signature field

1. Say: Some PDFs have no signature field. You can add one wherever the signature goes.
   Screen: open "Memo.pdf"; point "This PDF has no signature fields yet."
2. Say: Click the page at that spot, and a field of the usual size appears.
   Screen: place 1 432 398; picture "new-field"
3. Say: Drag the field to move it.
   Screen: drag "Signature1" -24 -6
4. Say: Drag the dot on its corner to change its size.
   Screen: resize "Signature1" 24 4
5. Say: Give it a name in the list. A field placed by mistake goes away with Delete, or with the cross next to it.
   Screen: rename "Signature1" "Author"
6. Say: Press Sign. The new field and your signature are saved together, with one PIN prompt.
   Screen: saveas "Memo signed.pdf"; click "Sign…"; finish
7. Say: Your signature goes where you placed the field, on any page.
   Screen: point ".box.signed"
