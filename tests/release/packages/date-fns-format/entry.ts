import { format } from "date-fns";

const date = new Date(2020, 0, 6, 13, 45, 30);

console.log(format(date, "yyyy-MM-dd"));
console.log(format(date, "yyyy-MM-dd HH:mm:ss"));
console.log(format(date, "MMMM do yyyy"));
console.log(format(date, "EEEE"));
console.log(format(date, "do"));
console.log(format(date, "Mo"));
console.log(format(date, "yo"));
console.log(format(date, "a"));
console.log(format(date, "aaa"));
console.log(format(date, "aaaa"));
console.log(format(date, "aaaaa"));
